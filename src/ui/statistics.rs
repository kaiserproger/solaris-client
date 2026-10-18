//! Authoritative statistics received through AwardStats, with presentation projections.
//! Wire/type/formatter contracts: local mapped 26.1.2 ClientboundAwardStatsPacket,
//! Stat, StatType, Stats and StatFormatter javap (local artifacts 13651/13654).
//! No registry numbers are embedded here; Content owns their connection mapping.
use crate::protocol::{Reader, Result, content::Content};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatisticsGroup {
    #[default]
    General,
    Items,
    Mobs,
}

#[derive(Default)]
pub(crate) struct StatisticsState {
    received: bool,
    counters: BTreeMap<String, BTreeMap<String, i32>>,
}

pub(crate) struct StatisticsRow {
    pub id: String,
    pub component: Value,
    pub value: i32,
    pub formatted: String,
}

impl StatisticsState {
    pub(crate) fn apply(&mut self, r: &mut Reader<'_>, content: &Content) -> Result<()> {
        // Stat.STREAM_CODEC dispatches on stat_type, then the StatType's registry.
        // AwardStats is a VarInt-sized map of these keys to absolute VarInt counters.
        let count = r.count(65536)?;
        let mut updates = Vec::with_capacity(count);
        for _ in 0..count {
            let stat_type = content.registry_name("minecraft:stat_type", r.var()?)?;
            let (registry, _) = stat_registry(stat_type)?;
            let stat = content.registry_name(registry, r.var()?)?;
            let counter = r.var()?;
            updates.push((stat_type, stat, counter));
        }
        // A malformed later entry or trailing bytes must not publish earlier entries,
        // nor release the request's waiting state. An empty accepted map does release it.
        r.done()?;
        for (stat_type, stat, counter) in updates {
            if let Some(counters) = self.counters.get_mut(stat_type) {
                if let Some(value) = counters.get_mut(stat) {
                    *value = counter;
                } else {
                    counters.insert(stat.to_owned(), counter);
                }
            } else {
                self.counters.insert(
                    stat_type.to_owned(),
                    BTreeMap::from([(stat.to_owned(), counter)]),
                );
            }
        }
        self.received = true;
        Ok(())
    }

    pub(crate) fn begin_request(&mut self) {
        // Reopening requests another authoritative update, not a local counter reset.
        self.received = false;
    }

    pub(crate) fn received(&self) -> bool {
        self.received
    }

    pub(crate) fn rows(
        &self,
        content: &Content,
        group: StatisticsGroup,
    ) -> Result<Vec<StatisticsRow>> {
        let mut rows = Vec::new();
        if group == StatisticsGroup::General {
            let custom = content.registry_entries("minecraft:custom_stat")?;
            rows.reserve(custom.len());
            let counters = self.counters.get("minecraft:custom");
            for (name, _) in custom {
                let value = counters.and_then(|c| c.get(name)).copied().unwrap_or(0);
                rows.push(stat_row("minecraft:custom", name, value)?);
            }
        } else {
            for (stat_type, counters) in &self.counters {
                if stat_registry(stat_type)?.1 == group {
                    for (name, value) in counters {
                        rows.push(stat_row(stat_type, name, *value)?);
                    }
                }
            }
        }
        rows.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }

    pub(crate) fn observation(&self, _content: &Content) -> Result<Value> {
        let mut counters = Vec::new();
        for (stat_type, values) in &self.counters {
            for (stat, value) in values {
                let row = stat_row(stat_type, stat, *value)?;
                counters.push(json!({
                    "id": row.id,
                    "stat_type": stat_type,
                    "stat": stat,
                    "value": row.value,
                    "formatted": row.formatted,
                    "component": row.component,
                    "group": stat_registry(stat_type)?.1,
                }));
            }
        }
        Ok(json!({"received": self.received, "counters": counters}))
    }
}

fn stat_registry(stat_type: &str) -> Result<(&'static str, StatisticsGroup)> {
    // Exact Stats.makeRegistryStatType registry associations, not wire IDs.
    Ok(match stat_type {
        "minecraft:custom" => ("minecraft:custom_stat", StatisticsGroup::General),
        "minecraft:mined" => ("minecraft:block", StatisticsGroup::Items),
        "minecraft:crafted"
        | "minecraft:used"
        | "minecraft:broken"
        | "minecraft:picked_up"
        | "minecraft:dropped" => ("minecraft:item", StatisticsGroup::Items),
        "minecraft:killed" | "minecraft:killed_by" => {
            ("minecraft:entity_type", StatisticsGroup::Mobs)
        }
        _ => return Err(format!("unknown statistics type {stat_type}")),
    })
}

fn stat_row(stat_type: &str, name: &str, value: i32) -> Result<StatisticsRow> {
    stat_registry(stat_type)?;
    let component = if stat_type == "minecraft:custom" {
        // StatsScreen.getTranslationKey: "stat." + identifier with ':' replaced by '.'.
        json!({"translate": format!("stat.{}", name.replace(':', "."))})
    } else {
        // Deliberately simple named-count rows, not the vanilla item icon/table view.
        // Keep the registry name verbatim rather than guessing an item's description ID.
        json!({"text": "", "extra": [
            {"translate": format!("stat_type.{}", stat_type.replace(':', "."))},
            {"text": ": "},
            {"text": name}
        ]})
    };
    Ok(StatisticsRow {
        id: format!("{stat_type}/{name}"),
        component,
        value,
        formatted: format_stat(stat_type, name, value),
    })
}

fn format_stat(stat_type: &str, name: &str, value: i32) -> String {
    if stat_type == "minecraft:custom" {
        // Exact Stats static-constructor formatter selections. Do not classify by
        // suffix: unrecognized and non-Minecraft custom statistics use DEFAULT.
        match name {
            "minecraft:play_time"
            | "minecraft:total_world_time"
            | "minecraft:time_since_death"
            | "minecraft:time_since_rest"
            | "minecraft:sneak_time" => return format_time(value),
            "minecraft:walk_one_cm"
            | "minecraft:crouch_one_cm"
            | "minecraft:sprint_one_cm"
            | "minecraft:walk_on_water_one_cm"
            | "minecraft:fall_one_cm"
            | "minecraft:climb_one_cm"
            | "minecraft:fly_one_cm"
            | "minecraft:walk_under_water_one_cm"
            | "minecraft:minecart_one_cm"
            | "minecraft:boat_one_cm"
            | "minecraft:pig_one_cm"
            | "minecraft:happy_ghast_one_cm"
            | "minecraft:horse_one_cm"
            | "minecraft:aviate_one_cm"
            | "minecraft:swim_one_cm"
            | "minecraft:strider_one_cm"
            | "minecraft:nautilus_one_cm" => return format_distance(value),
            "minecraft:damage_dealt"
            | "minecraft:damage_dealt_absorbed"
            | "minecraft:damage_dealt_resisted"
            | "minecraft:damage_taken"
            | "minecraft:damage_blocked_by_shield"
            | "minecraft:damage_absorbed"
            | "minecraft:damage_resisted" => return format!("{:.2}", f64::from(value) * 0.1),
            _ => {}
        }
    }
    format_integer(value)
}

fn format_integer(value: i32) -> String {
    // StatFormatter.DEFAULT: NumberFormat.getIntegerInstance(Locale.US).
    // The longest signed i32 needs ten digits, three separators and one sign.
    let mut bytes = [0u8; 14];
    let mut start = bytes.len();
    let mut remaining = value.unsigned_abs();
    let mut group = 0;
    loop {
        start -= 1;
        bytes[start] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        group += 1;
        if remaining == 0 {
            break;
        }
        if group == 3 {
            start -= 1;
            bytes[start] = b',';
            group = 0;
        }
    }
    if value < 0 {
        start -= 1;
        bytes[start] = b'-';
    }
    std::str::from_utf8(&bytes[start..]).unwrap().to_owned()
}

fn format_distance(value: i32) -> String {
    let meters = f64::from(value) / 100.0;
    let kilometers = meters / 1000.0;
    // DecimalFormat("########0.00", Locale.ROOT) uses HALF_EVEN, as does
    // Rust's fixed precision formatting of the computed binary f64 value.
    if kilometers > 0.5 {
        format!("{kilometers:.2} km")
    } else if meters > 0.5 {
        format!("{meters:.2} m")
    } else {
        format!("{value} cm")
    }
}

fn format_time(value: i32) -> String {
    // Preserve the source's successive double divisions and strict comparisons.
    let seconds = f64::from(value) / 20.0;
    let minutes = seconds / 60.0;
    let hours = minutes / 60.0;
    let days = hours / 24.0;
    let years = days / 365.0;
    if years > 0.5 {
        format!("{years:.2} y")
    } else if days > 0.5 {
        format!("{days:.2} d")
    } else if hours > 0.5 {
        format!("{hours:.2} h")
    } else if minutes > 0.5 {
        format!("{minutes:.2} min")
    } else if seconds.abs() >= 10_000_000.0 {
        // The seconds branch concatenates Double.toString, not DECIMAL_FORMAT.
        // Positive large values select another unit; negative counters still reach
        // Java's scientific-notation boundary. All inputs are integer ticks / 20.
        let scientific = format!("{seconds:e}");
        let (mantissa, exponent) = scientific.split_once('e').unwrap();
        if mantissa.contains('.') {
            format!("{mantissa}E{exponent} s")
        } else {
            format!("{mantissa}.0E{exponent} s")
        }
    } else if seconds.fract() == 0.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{seconds} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Writer;

    fn content() -> Content {
        let mut content = Content::test_cube();
        // Own registry fixtures deliberately differ from vanilla numeric order.
        let tables: &[(&str, &[&str])] = &[
            (
                "minecraft:stat_type",
                &[
                    "minecraft:used",
                    "minecraft:custom",
                    "minecraft:killed_by",
                    "minecraft:mined",
                    "minecraft:picked_up",
                    "minecraft:crafted",
                    "minecraft:dropped",
                    "minecraft:broken",
                    "minecraft:killed",
                    "test:unknown_type",
                ],
            ),
            (
                "minecraft:custom_stat",
                &[
                    "minecraft:jump",
                    "minecraft:walk_one_cm",
                    "minecraft:play_time",
                    "minecraft:damage_taken",
                    "test:walk_one_cm",
                ],
            ),
            ("minecraft:block", &["test:cube", "test:other_block"]),
            ("minecraft:item", &["test:other_item", "test:tool"]),
            ("minecraft:entity_type", &["test:mob", "test:other_mob"]),
        ];
        content.set_registries(
            tables
                .iter()
                .map(|(registry, names)| {
                    (
                        (*registry).to_owned(),
                        names
                            .iter()
                            .map(|name| ((*name).to_owned(), Value::Null))
                            .collect(),
                    )
                })
                .collect(),
        );
        content
    }

    fn id(content: &Content, registry: &str, name: &str) -> i32 {
        content
            .registry_entries(registry)
            .unwrap()
            .iter()
            .position(|(entry, _)| entry == name)
            .unwrap() as i32
    }

    fn packet(content: &Content, entries: &[(&str, &str, i32)]) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.var(entries.len() as i32);
        for (stat_type, stat, value) in entries {
            writer.var(id(content, "minecraft:stat_type", stat_type));
            writer.var(id(content, stat_registry(stat_type).unwrap().0, stat));
            writer.var(*value);
        }
        writer.0
    }

    fn apply(state: &mut StatisticsState, content: &Content, entries: &[(&str, &str, i32)]) {
        let packet = packet(content, entries);
        state.apply(&mut Reader::new(&packet), content).unwrap();
    }

    #[test]
    fn p1_statistics_packet_merges_absolute_counters_and_keeps_explicit_zero() {
        let content = content();
        let mut state = StatisticsState::default();
        apply(
            &mut state,
            &content,
            &[
                ("minecraft:custom", "minecraft:walk_one_cm", 50001),
                ("minecraft:used", "test:tool", 9),
            ],
        );
        apply(
            &mut state,
            &content,
            &[
                ("minecraft:used", "test:tool", 2),
                ("minecraft:custom", "minecraft:jump", 4),
                ("minecraft:used", "test:tool", 0),
            ],
        );
        assert!(state.received());
        assert_eq!(state.counters["minecraft:used"]["test:tool"], 0);
        assert_eq!(
            state.counters["minecraft:custom"]["minecraft:walk_one_cm"],
            50001
        );
        assert_eq!(state.counters["minecraft:custom"]["minecraft:jump"], 4);
        let rows = state.rows(&content, StatisticsGroup::Items).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].value, 0);
    }

    #[test]
    fn p1_statistics_request_empty_response_and_connection_reset() {
        let content = content();
        let mut state = StatisticsState::default();
        assert!(!state.received());
        apply(
            &mut state,
            &content,
            &[("minecraft:custom", "minecraft:jump", 7)],
        );
        state.begin_request();
        assert!(!state.received());
        assert_eq!(state.counters["minecraft:custom"]["minecraft:jump"], 7);
        apply(&mut state, &content, &[]);
        assert!(state.received());
        assert_eq!(state.counters["minecraft:custom"]["minecraft:jump"], 7);
        state = StatisticsState::default();
        assert!(!state.received());
        assert!(state.counters.is_empty());
        assert_eq!(state.observation(&content).unwrap()["counters"], json!([]));
    }

    #[test]
    fn p1_statistics_malformed_packets_are_transactional() {
        let content = content();
        let mut state = StatisticsState::default();
        apply(
            &mut state,
            &content,
            &[("minecraft:custom", "minecraft:jump", 7)],
        );
        let valid = packet(&content, &[("minecraft:custom", "minecraft:jump", 99)]);
        let mut malformed = Vec::new();
        let mut trailing = valid.clone();
        trailing.push(0);
        malformed.push(trailing);
        malformed.push(valid[..valid.len() - 1].to_vec());
        for (stat_type, stat) in [
            (-1, 0),
            (100, 0),
            (id(&content, "minecraft:stat_type", "minecraft:custom"), -1),
            (id(&content, "minecraft:stat_type", "minecraft:custom"), 100),
            (id(&content, "minecraft:stat_type", "test:unknown_type"), 0),
        ] {
            let mut writer = Writer::default();
            writer.var(2);
            writer.0.extend_from_slice(&valid[1..]);
            writer.var(stat_type);
            writer.var(stat);
            writer.var(3);
            malformed.push(writer.0);
        }
        for count in [-1, 65537] {
            let mut writer = Writer::default();
            writer.var(count);
            malformed.push(writer.0);
        }
        malformed.push(vec![0, 0]);
        malformed.push(vec![1, 0x80, 0x80, 0x80, 0x80, 0x10]);
        for waiting in [false, true] {
            if waiting {
                state.begin_request();
            }
            let before = state.observation(&content).unwrap();
            for bytes in &malformed {
                assert!(state.apply(&mut Reader::new(bytes), &content).is_err());
                assert_eq!(state.observation(&content).unwrap(), before);
            }
        }
    }

    #[test]
    fn p1_statistics_missing_value_registry_is_transactional() {
        let mut content = content();
        let mut state = StatisticsState::default();
        apply(
            &mut state,
            &content,
            &[("minecraft:custom", "minecraft:jump", 7)],
        );
        state.begin_request();
        let bytes = packet(&content, &[("minecraft:used", "test:tool", 8)]);
        let types = content
            .registry_entries("minecraft:stat_type")
            .unwrap()
            .to_vec();
        content.set_registries(BTreeMap::from([("minecraft:stat_type".into(), types)]));
        assert!(state.apply(&mut Reader::new(&bytes), &content).is_err());
        assert!(!state.received());
        assert_eq!(state.counters["minecraft:custom"]["minecraft:jump"], 7);
    }

    #[test]
    fn p1_statistics_all_named_types_resolve_their_own_registry() {
        let content = content();
        let mut state = StatisticsState::default();
        let entries = [
            ("minecraft:mined", "test:other_block", 1),
            ("minecraft:crafted", "test:tool", 2),
            ("minecraft:used", "test:tool", 3),
            ("minecraft:broken", "test:tool", 4),
            ("minecraft:picked_up", "test:tool", 5),
            ("minecraft:dropped", "test:tool", 6),
            ("minecraft:killed", "test:other_mob", 7),
            ("minecraft:killed_by", "test:other_mob", 8),
            ("minecraft:custom", "minecraft:jump", i32::MIN),
        ];
        apply(&mut state, &content, &entries);
        for (stat_type, stat, value) in entries {
            assert_eq!(state.counters[stat_type][stat], value);
        }
        assert_eq!(
            state.rows(&content, StatisticsGroup::Items).unwrap().len(),
            6
        );
        assert_eq!(
            state.rows(&content, StatisticsGroup::Mobs).unwrap().len(),
            2
        );
    }

    #[test]
    fn p1_statistics_projections_zero_default_and_observation_received_only() {
        let content = content();
        let mut state = StatisticsState::default();
        apply(
            &mut state,
            &content,
            &[
                ("minecraft:custom", "minecraft:walk_one_cm", 50001),
                ("minecraft:custom", "minecraft:damage_taken", 37),
                ("minecraft:used", "test:tool", 1234),
                ("minecraft:killed_by", "test:mob", 0),
            ],
        );
        let general = state.rows(&content, StatisticsGroup::General).unwrap();
        assert_eq!(general.len(), 5);
        let walk = general
            .iter()
            .find(|row| row.id.ends_with("/minecraft:walk_one_cm"))
            .unwrap();
        assert_eq!(
            walk.component,
            json!({"translate": "stat.minecraft.walk_one_cm"})
        );
        assert_eq!(walk.value, 50001);
        assert_eq!(walk.formatted, "0.50 km");
        let play = general
            .iter()
            .find(|row| row.id.ends_with("/minecraft:play_time"))
            .unwrap();
        assert_eq!(play.value, 0);
        assert_eq!(play.formatted, "0.0 s");
        let items = state.rows(&content, StatisticsGroup::Items).unwrap();
        assert_eq!(items[0].id, "minecraft:used/test:tool");
        assert_eq!(items[0].formatted, "1,234");
        assert_eq!(
            items[0].component["extra"][0]["translate"],
            "stat_type.minecraft.used"
        );
        assert_eq!(items[0].component["extra"][2]["text"], "test:tool");
        let mobs = state.rows(&content, StatisticsGroup::Mobs).unwrap();
        assert_eq!(mobs[0].id, "minecraft:killed_by/test:mob");
        assert_eq!(mobs[0].formatted, "0");
        let observed = state.observation(&content).unwrap();
        assert_eq!(observed["received"], true);
        let counters = observed["counters"].as_array().unwrap();
        assert_eq!(counters.len(), 4);
        assert!(
            !counters
                .iter()
                .any(|row| row["stat"] == "minecraft:play_time")
        );
        let damage = counters
            .iter()
            .find(|row| row["stat"] == "minecraft:damage_taken")
            .unwrap();
        assert_eq!(damage["value"], 37);
        assert_eq!(damage["formatted"], "3.70");
        assert_eq!(damage["group"], "general");
    }

    #[test]
    fn p1_statistics_exact_distance_thresholds_and_half_even() {
        for (value, expected) in [
            (0, "0 cm"),
            (50, "50 cm"),
            (51, "0.51 m"),
            (50000, "500.00 m"),
            (50001, "0.50 km"),
            (112500, "1.12 km"),
            (137500, "1.38 km"),
            (-51, "-51 cm"),
            (i32::MIN, "-2147483648 cm"),
            (i32::MAX, "21474.84 km"),
        ] {
            assert_eq!(format_distance(value), expected, "distance {value}");
        }
    }

    #[test]
    fn p1_statistics_exact_time_thresholds_seconds_suffixes_and_rounding() {
        for (value, expected) in [
            (0, "0.0 s"),
            (1, "0.05 s"),
            (20, "1.0 s"),
            (599, "29.95 s"),
            (600, "30.0 s"),
            (601, "0.50 min"),
            (36000, "30.00 min"),
            (36001, "0.50 h"),
            (864000, "12.00 h"),
            (864001, "0.50 d"),
            (315360000, "182.50 d"),
            (315360001, "0.50 y"),
            (1350, "1.12 min"),
            (1650, "1.38 min"),
            (-1, "-0.05 s"),
            (-20, "-1.0 s"),
            (-600, "-30.0 s"),
            (-200000000, "-1.0E7 s"),
            (i32::MIN, "-1.073741824E8 s"),
            (i32::MAX, "3.40 y"),
        ] {
            assert_eq!(format_time(value), expected, "time {value}");
        }
    }

    #[test]
    fn p1_statistics_exact_custom_formatter_mappings_and_defaults() {
        for name in [
            "play_time",
            "total_world_time",
            "time_since_death",
            "time_since_rest",
            "sneak_time",
        ] {
            assert_eq!(
                format_stat("minecraft:custom", &format!("minecraft:{name}"), 20),
                "1.0 s"
            );
        }
        for name in [
            "walk_one_cm",
            "crouch_one_cm",
            "sprint_one_cm",
            "walk_on_water_one_cm",
            "fall_one_cm",
            "climb_one_cm",
            "fly_one_cm",
            "walk_under_water_one_cm",
            "minecart_one_cm",
            "boat_one_cm",
            "pig_one_cm",
            "happy_ghast_one_cm",
            "horse_one_cm",
            "aviate_one_cm",
            "swim_one_cm",
            "strider_one_cm",
            "nautilus_one_cm",
        ] {
            assert_eq!(
                format_stat("minecraft:custom", &format!("minecraft:{name}"), 50),
                "50 cm"
            );
        }
        for name in [
            "damage_dealt",
            "damage_dealt_absorbed",
            "damage_dealt_resisted",
            "damage_taken",
            "damage_blocked_by_shield",
            "damage_absorbed",
            "damage_resisted",
        ] {
            assert_eq!(
                format_stat("minecraft:custom", &format!("minecraft:{name}"), 37),
                "3.70"
            );
        }
        for name in [
            "minecraft:jump",
            "minecraft:leave_game",
            "test:walk_one_cm",
            "minecraft:unknown_one_cm",
        ] {
            assert_eq!(format_stat("minecraft:custom", name, 1234), "1,234");
        }
        assert_eq!(
            format_stat("minecraft:used", "minecraft:play_time", 1234),
            "1,234"
        );
        assert_eq!(format_integer(i32::MIN), "-2,147,483,648");
        assert_eq!(format_integer(i32::MAX), "2,147,483,647");
        assert_eq!(
            format_stat("minecraft:custom", "minecraft:damage_taken", -37),
            "-3.70"
        );
        assert_eq!(
            format_stat("minecraft:custom", "minecraft:damage_taken", i32::MAX),
            "214748364.70"
        );
    }
}
