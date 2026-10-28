//! Received advancement graph and tab selection, shared by rendering and semantic input.
//! Local 26.1.2 javap: AdvancementsScreen.init/onAddAdvancementRoot/getTab,
//! AdvancementTab.create and ClientAdvancements.update/setSelectedTab. Tabs are
//! displayed roots, ancestry follows parents, and update order is reset/remove/add/progress.
//! Unlike vanilla's fixed edge-tab capacity, the native list exposes every received root.
use super::{packets::AdvancementState, state::UiState};
use crate::protocol::{Reader, Result};
use std::collections::{BTreeMap, BTreeSet};

impl UiState {
    /// All received roots with display information, including data-pack roots.
    pub fn advancement_tab_ids(&self) -> Vec<&str> {
        self.advancements
            .iter()
            .filter(|(_, advancement)| {
                advancement.parent.is_none() && advancement.display.is_some()
            })
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Resolve a received node to a displayed root without allocating a visited set.
    /// A path longer than the received graph necessarily contains a cycle.
    pub fn advancement_root_for(&self, id: &str) -> Option<&str> {
        let (mut id, mut advancement) = self.advancements.get_key_value(id)?;
        for _ in 0..self.advancements.len() {
            match advancement.parent.as_deref() {
                Some(parent) => (id, advancement) = self.advancements.get_key_value(parent)?,
                None => return advancement.display.as_ref().map(|_| id.as_str()),
            }
        }
        None
    }

    /// A valid received root selection, otherwise the first displayed received root.
    pub fn selected_advancement_root(&self) -> Option<&str> {
        self.selected_advancement
            .as_deref()
            .and_then(|id| self.displayed_advancement_root(id))
            .or_else(|| {
                self.advancements
                    .iter()
                    .find(|(_, advancement)| {
                        advancement.parent.is_none() && advancement.display.is_some()
                    })
                    .map(|(id, _)| id.as_str())
            })
    }

    pub fn select_advancement_tab(&mut self, id: &str) -> Result<()> {
        if self.displayed_advancement_root(id).is_none() {
            return Err("advancement_tab_not_observed".into());
        }
        if self.selected_advancement.as_deref() != Some(id) {
            self.selected_advancement = Some(id.to_owned());
        }
        self.scroll = 0;
        self.invalidate();
        Ok(())
    }

    fn displayed_advancement_root(&self, id: &str) -> Option<&str> {
        let (id, advancement) = self.advancements.get_key_value(id)?;
        (advancement.parent.is_none() && advancement.display.is_some()).then_some(id.as_str())
    }

    pub(super) fn read_advancement_tab(&mut self, r: &mut Reader<'_>) -> Result<()> {
        // ClientboundSelectAdvancementsTabPacket: nullable Identifier. Do not
        // publish a truncated/trailing packet, and never echo server selection.
        let selected = if r.bool()? {
            Some(r.string(32767)?)
        } else {
            None
        };
        r.done()?;
        self.selected_advancement =
            selected.filter(|id| self.displayed_advancement_root(id).is_some());
        self.scroll = 0;
        Ok(())
    }

    pub(super) fn apply_advancement_update(
        &mut self,
        reset: bool,
        added: Vec<(String, AdvancementState)>,
        removed: Vec<String>,
        progress: Vec<(String, BTreeMap<String, Option<i64>>)>,
    ) -> Result<()> {
        // AdvancementTree.remove recursively forgets descendants. Build the
        // closure iteratively so malformed cycles cannot recurse indefinitely.
        let mut forgotten: BTreeSet<String> = BTreeSet::new();
        if !reset && !removed.is_empty() {
            let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for (id, advancement) in &self.advancements {
                if let Some(parent) = advancement.parent.as_deref() {
                    children.entry(parent).or_default().push(id.as_str());
                }
            }
            let mut pending: Vec<&str> = removed
                .iter()
                .map(String::as_str)
                .filter(|id| self.advancements.contains_key(*id))
                .collect();
            while let Some(id) = pending.pop() {
                if !forgotten.contains(id) {
                    forgotten.insert(id.to_owned());
                    if let Some(descendants) = children.get(id) {
                        pending.extend(descendants.iter().copied());
                    }
                }
            }
        }
        // Retention failure must leave the entire prior graph and selection intact.
        // Only allocate the exact candidate-key set when the cheap upper bound fails.
        let retained = if reset {
            0
        } else {
            self.advancements.len() - forgotten.len()
        };
        if retained + added.len() > 4096 {
            let mut ids: BTreeSet<&str> = if reset {
                BTreeSet::new()
            } else {
                self.advancements
                    .keys()
                    .map(String::as_str)
                    .filter(|id| !forgotten.contains(*id))
                    .collect()
            };
            ids.extend(added.iter().map(|(id, _)| id.as_str()));
            if ids.len() > 4096 {
                return Err("advancement_retention_limit".into());
            }
        }
        // A scrolled list needs its old effective root only until publication.
        let old_root =
            (self.scroll != 0).then(|| self.selected_advancement_root().map(str::to_owned));
        if reset {
            self.advancements.clear();
            self.selected_advancement = None;
            self.scroll = 0;
        } else if !forgotten.is_empty() {
            self.advancements
                .retain(|id, _| !forgotten.contains(id.as_str()));
        }
        for (id, advancement) in added {
            self.advancements.insert(id, advancement);
        }
        for (id, criteria) in progress {
            if let Some(advancement) = self.advancements.get_mut(&id) {
                advancement.progress = criteria;
                advancement.done = !advancement.requirements.is_empty()
                    && advancement.requirements.iter().all(|group| {
                        group
                            .iter()
                            .any(|name| advancement.progress.get(name).is_some_and(Option::is_some))
                    });
            }
        }
        if self
            .selected_advancement
            .as_deref()
            .is_some_and(|id| self.displayed_advancement_root(id).is_none())
        {
            self.selected_advancement = None;
            self.scroll = 0;
        }
        if old_root.is_some_and(|old| old.as_deref() != self.selected_advancement_root()) {
            self.scroll = 0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Writer, content::Content};
    use serde_json::json;

    fn advancement(parent: Option<&str>, displayed: bool) -> AdvancementState {
        AdvancementState {
            parent: parent.map(str::to_owned),
            display: displayed.then(|| json!({"text": "Own advancement fixture"})),
            description: None,
            icon: None,
            background: None,
            frame: 0,
            flags: 0,
            position: [0.; 2],
            requirements: Vec::new(),
            progress: BTreeMap::new(),
            done: false,
            telemetry: false,
        }
    }

    fn state() -> (UiState, Content) {
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        let state = UiState::new_state(&content).unwrap();
        (state, content)
    }

    fn roots(state: &mut UiState) {
        for name in ["story", "nether", "end", "adventure", "husbandry"] {
            state
                .advancements
                .insert(format!("minecraft:{name}/root"), advancement(None, true));
        }
    }

    #[test]
    fn p1_advancements_five_roots_custom_tabs_and_child_ancestry() {
        let (mut state, _) = state();
        roots(&mut state);
        state
            .advancements
            .insert("custom:root".into(), advancement(None, true));
        state
            .advancements
            .insert("custom:hidden_root".into(), advancement(None, false));
        state.advancements.insert(
            "custom:branch".into(),
            advancement(Some("custom:root"), false),
        );
        state.advancements.insert(
            "custom:leaf".into(),
            advancement(Some("custom:branch"), true),
        );
        // More than sixteen alphabetically earlier descendants cannot hide roots.
        for i in 0..32 {
            state.advancements.insert(
                format!("aaa:child_{i}"),
                advancement(Some("minecraft:story/root"), true),
            );
        }
        assert_eq!(state.advancement_tab_ids().len(), 6);
        for name in ["story", "nether", "end", "adventure", "husbandry"] {
            let id = format!("minecraft:{name}/root");
            assert!(state.advancement_tab_ids().contains(&id.as_str()));
            state.scroll = 12;
            let revision = state.revision;
            state.select_advancement_tab(&id).unwrap();
            assert_eq!(state.selected_advancement_root(), Some(id.as_str()));
            assert_eq!(state.scroll, 0);
            assert!(state.revision > revision);
        }
        assert_eq!(
            state.advancement_root_for("custom:leaf"),
            Some("custom:root")
        );
        assert_eq!(state.advancement_root_for("custom:hidden_root"), None);
        state.select_advancement_tab("custom:root").unwrap();
        assert_eq!(state.selected_advancement_root(), Some("custom:root"));
        let revision = state.revision;
        assert!(state.select_advancement_tab("custom:leaf").is_err());
        assert_eq!(state.selected_advancement_root(), Some("custom:root"));
        assert_eq!(state.revision, revision);
    }

    #[test]
    fn p1_advancements_missing_parent_and_cycle_are_not_roots() {
        let (mut state, _) = state();
        state.advancements.insert(
            "test:orphan".into(),
            advancement(Some("test:missing"), true),
        );
        state
            .advancements
            .insert("test:a".into(), advancement(Some("test:b"), true));
        state
            .advancements
            .insert("test:b".into(), advancement(Some("test:a"), true));
        state
            .advancements
            .insert("test:self".into(), advancement(Some("test:self"), true));
        for id in [
            "test:missing",
            "test:orphan",
            "test:a",
            "test:b",
            "test:self",
        ] {
            assert_eq!(state.advancement_root_for(id), None);
        }
        assert!(state.advancement_tab_ids().is_empty());
        assert_eq!(state.selected_advancement_root(), None);
        state
            .advancements
            .insert("test:missing".into(), advancement(None, true));
        assert_eq!(
            state.advancement_root_for("test:orphan"),
            Some("test:missing")
        );
    }

    #[test]
    fn p1_advancements_received_selection_is_transactional_and_resets_scroll() {
        let (mut state, content) = state();
        roots(&mut state);
        state
            .select_advancement_tab("minecraft:story/root")
            .unwrap();
        let mut packet = Writer::default();
        packet.byte(1);
        packet.string("minecraft:nether/root");
        state.scroll = 7;
        let revision = state.revision;
        let mut trailing = packet.0.clone();
        trailing.push(0);
        assert!(
            state
                .apply(87, &mut Reader::new(&trailing), &content)
                .is_err()
        );
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:story/root")
        );
        assert_eq!(state.scroll, 7);
        assert_eq!(state.revision, revision);
        assert!(
            state
                .apply(
                    87,
                    &mut Reader::new(&packet.0[..packet.0.len() - 1]),
                    &content
                )
                .is_err()
        );
        assert_eq!(state.scroll, 7);
        state
            .apply(87, &mut Reader::new(&packet.0), &content)
            .unwrap();
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:nether/root")
        );
        assert_eq!(state.scroll, 0);
        assert!(state.revision > revision);
        assert!(state.take_packet_command().is_none());
        for id in [Some("test:missing"), None] {
            let mut packet = Writer::default();
            packet.byte(u8::from(id.is_some()));
            if let Some(id) = id {
                packet.string(id);
            }
            state.scroll = 9;
            state
                .apply(87, &mut Reader::new(&packet.0), &content)
                .unwrap();
            assert_eq!(state.selected_advancement, None);
            assert_eq!(
                state.selected_advancement_root(),
                Some("minecraft:adventure/root")
            );
            assert_eq!(state.scroll, 0);
        }
    }

    #[test]
    fn p1_advancements_remove_descendants_reset_and_readd_root() {
        let (mut state, content) = state();
        roots(&mut state);
        state.advancements.insert(
            "test:child".into(),
            advancement(Some("minecraft:story/root"), true),
        );
        state
            .advancements
            .insert("test:leaf".into(), advancement(Some("test:child"), true));
        state
            .select_advancement_tab("minecraft:story/root")
            .unwrap();
        state.scroll = 5;
        let mut packet = Writer::default();
        packet.byte(0); // no reset
        packet.var(0); // no additions
        packet.var(1);
        packet.string("minecraft:story/root");
        packet.var(0); // no progress
        packet.byte(0); // no toast
        state
            .apply(133, &mut Reader::new(&packet.0), &content)
            .unwrap();
        assert!(!state.advancements.contains_key("test:child"));
        assert!(!state.advancements.contains_key("test:leaf"));
        assert_eq!(state.selected_advancement, None);
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:adventure/root")
        );
        assert_eq!(state.scroll, 0);
        state
            .apply_advancement_update(
                false,
                vec![("minecraft:story/root".into(), advancement(None, true))],
                vec![],
                vec![],
            )
            .unwrap();
        state
            .select_advancement_tab("minecraft:story/root")
            .unwrap();
        state.scroll = 8;
        // Reset with no replacements clears selection as well as the graph.
        state
            .apply(133, &mut Reader::new(&[1, 0, 0, 0, 0]), &content)
            .unwrap();
        assert!(state.advancements.is_empty());
        assert_eq!(state.selected_advancement_root(), None);
        assert_eq!(state.selected_advancement, None);
        assert_eq!(state.scroll, 0);
        roots(&mut state);
        state.select_advancement_tab("minecraft:end/root").unwrap();
        state.clear_session();
        assert!(state.advancements.is_empty());
        assert_eq!(state.selected_advancement, None);
    }

    #[test]
    fn p1_advancements_packet_adds_out_of_order_ancestry_and_progress() {
        let (mut state, content) = state();
        roots(&mut state);
        state
            .select_advancement_tab("minecraft:story/root")
            .unwrap();
        state.scroll = 4;
        let mut packet = Writer::default();
        packet.byte(0);
        packet.var(2);
        // A leaf can arrive before its intermediate parent in the same packet.
        for (id, parent) in [
            ("test:leaf", "test:branch"),
            ("test:branch", "minecraft:story/root"),
        ] {
            packet.string(id);
            packet.byte(1);
            packet.string(parent);
            packet.byte(0); // no display information
            packet.var(1); // one requirement group
            packet.var(1);
            packet.string("test:criterion");
            packet.byte(0); // no telemetry
        }
        packet.var(0); // no removals
        packet.var(1); // progress for one advancement
        packet.string("test:leaf");
        packet.var(1);
        packet.string("test:criterion");
        packet.byte(1);
        packet.i64(1234);
        packet.byte(0); // no toast
        let mut malformed = packet.0.clone();
        malformed.push(0);
        assert!(
            state
                .apply(133, &mut Reader::new(&malformed), &content)
                .is_err()
        );
        assert!(!state.advancements.contains_key("test:leaf"));
        state
            .apply(133, &mut Reader::new(&packet.0), &content)
            .unwrap();
        assert_eq!(
            state.advancement_root_for("test:leaf"),
            Some("minecraft:story/root")
        );
        assert!(state.advancements["test:leaf"].done);
        assert_eq!(
            state.advancements["test:leaf"].progress["test:criterion"],
            Some(1234)
        );
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:story/root")
        );
        assert_eq!(state.scroll, 4); // progress/children do not change the selected tab
        state
            .apply_advancement_update(false, vec![], vec!["test:branch".into()], vec![])
            .unwrap();
        assert!(!state.advancements.contains_key("test:branch"));
        assert!(!state.advancements.contains_key("test:leaf"));
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:story/root")
        );
        assert_eq!(state.scroll, 4);
    }

    #[test]
    fn p1_advancements_retention_and_malformed_updates_do_not_publish() {
        let (mut state, content) = state();
        roots(&mut state);
        state
            .select_advancement_tab("minecraft:story/root")
            .unwrap();
        state.scroll = 6;
        let revision = state.revision;
        let before = state.advancements.len();
        assert!(
            state
                .apply(133, &mut Reader::new(&[1, 0, 0, 0, 0, 1]), &content)
                .is_err()
        );
        assert_eq!(state.advancements.len(), before);
        assert_eq!(state.revision, revision);
        let added = (0..4096)
            .map(|i| (format!("test:root_{i}"), advancement(None, true)))
            .collect();
        assert!(
            state
                .apply_advancement_update(false, added, vec!["minecraft:end/root".into()], vec![])
                .is_err()
        );
        assert_eq!(state.advancements.len(), before);
        assert!(state.advancements.contains_key("minecraft:end/root"));
        assert_eq!(
            state.selected_advancement_root(),
            Some("minecraft:story/root")
        );
        assert_eq!(state.scroll, 6);
    }
}
