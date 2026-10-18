use crate::protocol::{Result, content::Content};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextStyle {
    pub color: [f32; 3],
    pub color_override: bool,
    pub shadow_color: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
    pub click: Option<Arc<Value>>,
    pub hover: Option<Arc<Value>>,
    pub font: Arc<str>,
}
impl Default for TextStyle {
    fn default() -> Self {
        static DEFAULT_FONT: LazyLock<Arc<str>> = LazyLock::new(|| Arc::from("minecraft:default"));
        Self {
            color: [1.; 3],
            color_override: false,
            shadow_color: None,
            bold: false,
            italic: false,
            underlined: false,
            strikethrough: false,
            obfuscated: false,
            click: None,
            hover: None,
            font: Arc::clone(&DEFAULT_FONT),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct TextRun {
    pub text: String,
    pub style: TextStyle,
}
pub struct Language {
    pub entries: BTreeMap<String, String>,
}
impl Language {
    pub fn load(c: &Content, language: &str) -> Result<Self> {
        if !language.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err("invalid language ID".into());
        }
        let bytes = c.resource_bytes(&format!("assets/minecraft/lang/{language}.json"))?;
        let entries = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        Ok(Self { entries })
    }
    pub fn translate<'a>(&'a self, key: &'a str) -> &'a str {
        self.entries.get(key).map(String::as_str).unwrap_or(key)
    }
    pub fn runs(&self, value: &Value) -> Result<Vec<TextRun>> {
        let mut out = Vec::new();
        self.visit(value, &TextStyle::default(), 0, &mut out)?;
        let total: usize = out.iter().map(|r| r.text.len()).sum();
        if total > 65536 {
            return Err("text component byte bound".into());
        }
        Ok(out)
    }
    pub fn plain(&self, value: &Value) -> Result<String> {
        Ok(self.runs(value)?.into_iter().map(|r| r.text).collect())
    }
    fn visit(
        &self,
        v: &Value,
        parent: &TextStyle,
        depth: u8,
        out: &mut Vec<TextRun>,
    ) -> Result<()> {
        if depth > 32 || out.len() > 4096 {
            return Err("text component complexity bound".into());
        }
        if let Some(text) = v.as_str() {
            out.push(TextRun {
                text: text.into(),
                style: parent.clone(),
            });
            return Ok(());
        }
        if let Some(list) = v.as_array() {
            for part in list {
                self.visit(part, parent, depth + 1, out)?;
            }
            return Ok(());
        }
        let obj = v
            .as_object()
            .ok_or("text component must be string, list, or object")?;
        let mut style = parent.clone();
        if let Some(color) = obj.get("color").and_then(Value::as_str) {
            style.color = parse_color(color)?;
            style.color_override = true;
        }
        if let Some(color) = obj.get("shadow_color") {
            style.shadow_color = Some(super::codec_int(color)? as u32);
        }
        if let Some(font) = obj.get("font") {
            let font = font.as_str().ok_or("text font resource ID")?;
            style.font = Arc::from(crate::protocol::parse_identifier(font)?);
        }
        for (name, target) in [
            ("bold", &mut style.bold),
            ("italic", &mut style.italic),
            ("underlined", &mut style.underlined),
            ("strikethrough", &mut style.strikethrough),
            ("obfuscated", &mut style.obfuscated),
        ] {
            if let Some(value) = obj.get(name) {
                *target = value
                    .as_bool()
                    .or_else(|| value.as_i64().map(|x| x != 0))
                    .ok_or("invalid text style boolean")?;
            }
        }
        if let Some(event) = obj.get("click_event").or_else(|| obj.get("clickEvent")) {
            style.click = Some(Arc::new(event.clone()));
        }
        if let Some(event) = obj.get("hover_event").or_else(|| obj.get("hoverEvent")) {
            style.hover = Some(Arc::new(event.clone()));
        }
        if let Some(text) = obj.get("text").and_then(Value::as_str) {
            out.push(TextRun {
                text: text.into(),
                style: style.clone(),
            });
        } else if let Some(key) = obj.get("translate").and_then(Value::as_str) {
            let template = self
                .entries
                .get(key)
                .map(String::as_str)
                .or_else(|| obj.get("fallback").and_then(Value::as_str))
                .unwrap_or(key);
            let args = obj
                .get("with")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut index = 0;
            let mut rest = template;
            while let Some(pos) = rest.find('%') {
                if pos > 0 {
                    out.push(TextRun {
                        text: rest[..pos].into(),
                        style: style.clone(),
                    });
                }
                rest = &rest[pos + 1..];
                if let Some(tail) = rest.strip_prefix('%') {
                    out.push(TextRun {
                        text: "%".into(),
                        style: style.clone(),
                    });
                    rest = tail;
                    continue;
                }
                let end = rest.find('s').ok_or("invalid translation format")?;
                let fmt = &rest[..end];
                let arg = if fmt.is_empty() {
                    let i = index;
                    index += 1;
                    i
                } else {
                    fmt.strip_suffix('$')
                        .ok_or("invalid translation placeholder")?
                        .parse::<usize>()
                        .map_err(|_| "translation argument number")?
                        .checked_sub(1)
                        .ok_or("translation argument zero")?
                };
                let value = args.get(arg).ok_or("missing translation argument")?;
                if value.is_number() || value.is_boolean() {
                    out.push(TextRun {
                        text: value.to_string(),
                        style: style.clone(),
                    });
                } else {
                    self.visit(value, &style, depth + 1, out)?;
                }
                rest = &rest[end + 1..];
            }
            if !rest.is_empty() {
                out.push(TextRun {
                    text: rest.into(),
                    style: style.clone(),
                });
            }
        } else if let Some(key) = obj.get("keybind").and_then(Value::as_str) {
            out.push(TextRun {
                text: self.translate(key).into(),
                style: style.clone(),
            });
        } else {
            return Err("unsupported dynamic score/selector/NBT text component without resolved server content".into());
        }
        if let Some(extra) = obj.get("extra").and_then(Value::as_array) {
            for part in extra {
                self.visit(part, &style, depth + 1, out)?;
            }
        }
        Ok(())
    }
}
fn parse_color(s: &str) -> Result<[f32; 3]> {
    let value = if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return Err("text RGB color length".into());
        }
        u32::from_str_radix(hex, 16).map_err(|_| "text RGB digits")?
    } else {
        match s {
            "black" => 0x000000,
            "dark_blue" => 0x0000aa,
            "dark_green" => 0x00aa00,
            "dark_aqua" => 0x00aaaa,
            "dark_red" => 0xaa0000,
            "dark_purple" => 0xaa00aa,
            "gold" => 0xffaa00,
            "gray" => 0xaaaaaa,
            "dark_gray" => 0x555555,
            "blue" => 0x5555ff,
            "green" => 0x55ff55,
            "aqua" => 0x55ffff,
            "red" => 0xff5555,
            "light_purple" => 0xff55ff,
            "yellow" => 0xffff55,
            "white" => 0xffffff,
            _ => return Err("unknown text color".into()),
        }
    };
    Ok([
        ((value >> 16) & 255) as f32 / 255.,
        ((value >> 8) & 255) as f32 / 255.,
        (value & 255) as f32 / 255.,
    ])
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct TextEntry {
    pub value: String,
    pub selection: [usize; 2],
    pub composition: String,
}
impl TextEntry {
    pub fn insert(&mut self, text: &str, max_utf16: usize) -> Result<()> {
        if text.chars().any(|c| c == '\0' || c == '\u{7f}') {
            return Err("invalid text entry control".into());
        }
        let [a, b] = self.selection;
        let start = a.min(b);
        let end = a.max(b);
        if end > self.value.len()
            || !self.value.is_char_boundary(start)
            || !self.value.is_char_boundary(end)
        {
            return Err("text selection boundary".into());
        }
        let length = self.value[..start].encode_utf16().count()
            + text.encode_utf16().count()
            + self.value[end..].encode_utf16().count();
        if length > max_utf16 {
            return Err("text entry character bound".into());
        }
        self.value.replace_range(start..end, text);
        let caret = start + text.len();
        self.selection = [caret, caret];
        self.composition.clear();
        Ok(())
    }
    pub fn select(&mut self, start: usize, end: usize) -> Result<()> {
        if end > self.value.len()
            || start > self.value.len()
            || !self.value.is_char_boundary(start)
            || !self.value.is_char_boundary(end)
        {
            return Err("text selection boundary".into());
        }
        self.selection = [start, end];
        Ok(())
    }
    pub fn backspace(&mut self) {
        if self.selection[0] == self.selection[1] {
            let caret = self.selection[0];
            if let Some((prev, _)) = self.value[..caret].char_indices().next_back() {
                self.selection[0] = prev;
            }
        }
        let _ = self.insert("", usize::MAX);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_never_splits_unicode() {
        let mut t = TextEntry::default();
        t.insert("Привет世界", 32).unwrap();
        assert!(t.select(1, 2).is_err());
        t.backspace();
        assert_eq!(t.value, "Привет世");
    }
    #[test]
    fn translation_preserves_argument_styles() {
        let l = Language {
            entries: BTreeMap::from([("test".into(), "%2$s / %1$s".into())]),
        };
        let runs = l
            .runs(&serde_json::json!({"translate":"test","with":["A",{"text":"B","bold":true}]}))
            .unwrap();
        assert_eq!(runs[0].text, "B");
        assert!(runs[0].style.bold);
        assert_eq!(
            l.plain(&serde_json::json!({"translate":"test","with":["A","B"]}))
                .unwrap(),
            "B / A"
        );
    }
}

/// ItemStack.getHoverName followed by the pinned Item.getName overrides.
pub fn item_name_component<'a>(
    content: &Content,
    stack: &'a crate::inventory::Stack,
) -> Result<std::borrow::Cow<'a, Value>> {
    use std::borrow::Cow;
    if let Some(name) = stack.components.get("minecraft:custom_name") {
        return Ok(Cow::Borrowed(name));
    }
    if let Some(book) = stack.components.get("minecraft:written_book_content") {
        let title = book.get("title").ok_or("written book title missing")?;
        let raw = title.get("raw").unwrap_or(title);
        let text = raw.as_str().ok_or("written book raw title")?;
        if !java_blank(text) {
            return Ok(Cow::Owned(serde_json::json!({"text":text})));
        }
    }
    static EMPTY: LazyLock<Value> = LazyLock::new(|| serde_json::json!({"text":""}));
    let base = || {
        Cow::Borrowed(
            stack
                .components
                .get("minecraft:item_name")
                .unwrap_or(&EMPTY),
        )
    };
    let description = content.item_description_id(stack.id)?;
    match content
        .item_name_owner(stack.id)?
        .rsplit('.')
        .next()
        .ok_or("item name owner missing")?
    {
        "Item" => Ok(base()),
        "AirItem" => Ok(Cow::Owned(serde_json::json!({"translate":description}))),
        "CompassItem" => Ok(
            if stack.components.contains_key("minecraft:lodestone_tracker") {
                Cow::Owned(serde_json::json!({"translate":"item.minecraft.lodestone_compass"}))
            } else {
                base()
            },
        ),
        "ShieldItem" => Ok(
            if let Some(color) = stack.components.get("minecraft:base_color") {
                Cow::Owned(
                    serde_json::json!({"translate":format!("{description}.{}", color.as_str().ok_or("shield base color")?)}),
                )
            } else {
                base()
            },
        ),
        "PlayerHeadItem" => Ok(
            if let Some(name) = stack
                .components
                .get("minecraft:profile")
                .and_then(|profile| profile.get("name"))
                .and_then(Value::as_str)
            {
                Cow::Owned(
                    serde_json::json!({"translate":format!("{description}.named"),"with":[name]}),
                )
            } else {
                base()
            },
        ),
        "PotionItem" | "TippedArrowItem" => {
            let Some(potion) = stack.components.get("minecraft:potion_contents") else {
                return Ok(base());
            };
            let name = if let Some(custom) = potion.get("custom_name") {
                custom.as_str().ok_or("potion custom name")?
            } else if let Some(holder) = potion.get("potion") {
                let holder = holder.as_str().ok_or("potion holder name")?;
                let value = content
                    .registry_entries("minecraft:potion")?
                    .iter()
                    .find(|(name, _)| name == holder)
                    .map(|(_, value)| value)
                    .ok_or_else(|| format!("potion name holder not observed: {holder}"))?;
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("verified potion name metadata unavailable")?
            } else {
                "empty"
            };
            Ok(Cow::Owned(
                serde_json::json!({"translate":format!("{description}.effect.{name}")}),
            ))
        }
        owner => Err(format!("unregistered verified item name owner: {owner}")),
    }
}
fn java_blank(text: &str) -> bool {
    text.chars().all(|ch| {
        matches!(ch,
            '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{0020}' | '\u{1680}' |
            '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}' |
            '\u{2029}' | '\u{205f}' | '\u{3000}'
        )
    })
}
