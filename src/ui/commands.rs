//! Java 26.3 command tree and chat completions. Wire facts: local vanilla-26.3/client.jar,
//! ClientboundCommandsPacket, ArgumentTypeInfos and command suggestion packet codecs (javap).
use super::{state::{Screen, UiCommand, UiEffects, UiState}, edit::EditBuffer};
use crate::protocol::{Reader, Result, content::Content, packet_ids::play::clientbound as cb};

#[derive(Default)]
pub(crate) struct CommandState {
    nodes: Vec<Node>, root: usize, next_id: i32,
    pending: Option<Pending>, cycle: Option<Cycle>,
}
struct Node { children: Vec<usize>, redirect: Option<usize>, kind: Kind }
enum Kind { Root, Literal(String), Argument }
struct Pending { id: i32, text: String, cursor: usize, backwards: bool }
struct Cycle { original: String, start: usize, end: usize, values: Vec<String>, index: Option<usize>, current: String, cursor: usize }

fn argument_properties(r: &mut Reader<'_>, id: i32) -> Result<()> {
    match id {
        0 | 7..=30 | 32..=42 | 49..=61 => {},
        1..=4 => {
            let flags = r.u8()?;
            if flags & !3 != 0 { return Err("invalid command numeric flags".into()); }
            for mask in [1, 2] { if flags & mask != 0 { match id {
                1 => { r.f32()?; }, 2 => { r.f64()?; }, 3 => { r.i32()?; }, 4 => { r.i64()?; }, _ => unreachable!(),
            } } }
        },
        5 => { if !(0..=2).contains(&r.var()?) { return Err("invalid command string type".into()); } },
        6 | 31 => { r.u8()?; },
        43 => { r.i32()?; },
        44..=48 => { r.string(32767)?; },
        _ => return Err(format!("unknown 26.3 command argument type {id}")),
    }
    Ok(())
}

fn read_tree(r: &mut Reader<'_>) -> Result<(Vec<Node>, usize)> {
    let n = r.count(65536)?;
    if n == 0 { return Err("empty command tree".into()); }
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = r.u8()?;
        let count = r.count(n)?;
        let mut children = Vec::with_capacity(count);
        for _ in 0..count { children.push(r.count(n - 1)?); }
        let redirect = if flags & 8 != 0 { Some(r.count(n - 1)?) } else { None };
        let kind = match flags & 3 {
            0 => Kind::Root,
            1 => Kind::Literal(r.string(32767)?),
            2 => { r.string(32767)?; let id = r.var()?; argument_properties(r, id)?; if flags & 16 != 0 { r.string(32767)?; } Kind::Argument },
            _ => return Err("invalid command node type".into()),
        };
        nodes.push(Node { children, redirect, kind });
    }
    let root = r.count(n - 1)?;
    if !matches!(nodes[root].kind, Kind::Root) { return Err("invalid command root".into()); }
    r.done()?;
    Ok((nodes, root))
}

fn utf16_offset(text: &str, units: usize) -> Option<usize> {
    let mut count = 0;
    for (byte, ch) in text.char_indices() {
        if count == units { return Some(byte); }
        count += ch.len_utf16();
        if count > units { return None; }
    }
    (count == units).then_some(text.len())
}

impl CommandState {
    fn candidates(&self, text: &str, cursor: usize) -> (usize, Vec<String>, bool) {
        if self.nodes.is_empty() || !text.starts_with('/') || cursor == 0 { return (cursor, Vec::new(), false); }
        let input = &text[1..cursor];
        let start = input.rfind(' ').map_or(1, |i| i + 2);
        let prefix = &text[start..cursor];
        let mut active = vec![self.root];
        for word in text[1..start].split_whitespace() {
            let mut next = Vec::new();
            for &index in &active {
                let node = &self.nodes[index];
                let parent = node.redirect.unwrap_or(index);
                for &child in &self.nodes[parent].children {
                    match &self.nodes[child].kind {
                        Kind::Literal(name) if name == word => next.push(child),
                        Kind::Argument => next.push(child),
                        _ => {},
                    }
                }
            }
            next.sort_unstable();
            next.dedup();
            if next.is_empty() { return (start, Vec::new(), true); }
            active = next;
        }
        let mut values = Vec::new();
        let mut dynamic = false;
        for index in active {
            let node = &self.nodes[index];
            let parent = node.redirect.unwrap_or(index);
            for &child in &self.nodes[parent].children {
                match &self.nodes[child].kind {
                    Kind::Literal(name) if name.get(..prefix.len()).is_some_and(|s| s.eq_ignore_ascii_case(prefix)) => values.push(name.clone()),
                    Kind::Argument => dynamic = true,
                    _ => {},
                }
            }
        }
        values.sort(); values.dedup();
        (start, values, dynamic)
    }
    fn apply_cycle(&mut self, field: &mut EditBuffer, backwards: bool) -> Result<bool> {
        let Some(c) = self.cycle.as_mut() else { return Ok(false); };
        if field.text != c.current || field.cursor != c.cursor || field.cursor != field.anchor { self.cycle = None; return Ok(false); }
        let n = c.values.len();
        if n == 0 { return Ok(false); }
        let index = match c.index {
            None if backwards => n - 1,
            None => 0,
            Some(i) if backwards => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        let value = &c.values[index];
        let prefix = &c.original[..c.start];
        let suffix = &c.original[c.end..];
        // A valid suggestion can exceed the local editor limit once the
        // untouched prefix and suffix are included. Do not mutate selection
        // or advance the cycle when it cannot be inserted.
        if prefix.len() + value.len() + suffix.len() > field.max_bytes
            || field.max_chars.is_some_and(|max|
                prefix.encode_utf16().count() + value.encode_utf16().count()
                    + suffix.encode_utf16().count() > max)
        {
            if prefix.chars().chain(value.chars()).chain(suffix.chars())
                .any(|ch| ch == '\0' || (!field.multiline && (ch == '\n' || ch == '\r')))
            {
                return Err("invalid_text_input".into());
            }
            return Ok(true);
        }
        let mut text = c.original.clone();
        text.replace_range(c.start..c.end, value);
        field.select(0, field.text.len())?;
        field.insert(&text)?;
        let cursor = c.start + c.values[index].len();
        field.select(cursor, cursor)?;
        c.current = text;
        c.cursor = cursor;
        c.index = Some(index);
        Ok(true)
    }
}

impl UiState {
    pub(crate) fn apply_command_packet(&mut self, id: i32, r: &mut Reader<'_>, _content: &Content) -> Result<bool> {
        match id {
            cb::COMMANDS => {
                let (nodes, root) = read_tree(r)?;
                self.command_completion.nodes = nodes;
                self.command_completion.root = root;
                self.command_completion.pending = None;
                self.command_completion.cycle = None;
            }
            cb::COMMAND_SUGGESTIONS => {
                let id = r.var()?;
                let start = r.count(32767)?;
                let len = r.count(32767)?;
                let count = r.count(4096)?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count { values.push(r.string(32767)?); if r.bool()? { r.nbt()?; } }
                r.done()?;
                let Some(pending) = self.command_completion.pending.take() else { return Ok(true); };
                if id != pending.id { self.command_completion.pending = Some(pending); return Ok(true); }
                let Some(field) = self.fields.get_mut("chat/input") else { return Ok(true); };
                if self.screen != Screen::Chat || field.text != pending.text || field.cursor != pending.cursor || field.anchor != field.cursor { return Ok(true); }
                let Some(end_units) = start.checked_add(len) else { return Err("suggestion range overflow".into()); };
                let (Some(start), Some(end)) = (utf16_offset(&pending.text, start), utf16_offset(&pending.text, end_units)) else { return Err("invalid suggestion UTF-16 range".into()); };
                if end > pending.cursor || values.is_empty() { return Ok(true); }
                self.command_completion.cycle = Some(Cycle { original: pending.text.clone(), start, end, values, index: None, current: pending.text, cursor: pending.cursor });
                self.command_completion.apply_cycle(field, pending.backwards)?;
                self.invalidate();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub(crate) fn complete_chat(&mut self, backwards: bool, effects: &mut UiEffects) -> Result<()> {
        let Some(field) = self.fields.get_mut("chat/input") else { return Ok(()); };
        if self.command_completion.apply_cycle(field, backwards)? { self.invalidate(); return Ok(()); }
        if field.anchor != field.cursor { return Ok(()); }
        let (start, values, dynamic) = self.command_completion.candidates(&field.text, field.cursor);
        if dynamic {
            let id = self.command_completion.next_id;
            self.command_completion.next_id = id.wrapping_add(1);
            let text = field.text.clone();
            self.command_completion.pending = Some(Pending { id, text: text.clone(), cursor: field.cursor, backwards });
            effects.commands.push(UiCommand::CommandSuggestion { id, command: text[..field.cursor].to_owned() });
        } else if !values.is_empty() {
            self.command_completion.pending = None;
            let text = field.text.clone();
            self.command_completion.cycle = Some(Cycle { original: text.clone(), start, end: field.cursor, values, index: None, current: text, cursor: field.cursor });
            self.command_completion.apply_cycle(field, backwards)?;
            self.invalidate();
        }
        Ok(())
    }
}
