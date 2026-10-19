use super::*;

impl Player {
    fn has_projectile(
        &self,
        inventory: &InventoryStore,
        content: &Content,
        crossbow: bool,
    ) -> Result<bool> {
        if self.abilities.instabuild {
            return Ok(true);
        }
        for hand in [1, 0] {
            let item = super::super::interaction::held(inventory, hand);
            if item_tag(item, content, "minecraft:arrows")?
                || crossbow && item_name(item, content)? == "minecraft:firework_rocket"
            {
                return Ok(true);
            }
        }
        for item in inventory
            .player
            .slots
            .get(9..45)
            .ok_or("player inventory layout")?
        {
            if item_tag(item.as_ref(), content, "minecraft:arrows")? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(in crate::player) fn air_result(
        &mut self,
        hand: u8,
        target: Option<&Hit>,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(item) =
            super::super::interaction::held(inventory, hand).filter(|item| !item.empty())
        else {
            return Ok(Outcome::Pass);
        };
        let duration = self.use_duration(item, content)?;
        let prediction_start = self.use_predictions.len();
        let result = match short_class(content.item_use_owner(item.id, false)?) {
            "Item" => self.generic_item_use(hand, world, content, inventory, entities, tick)?,
            "BowItem" => {
                if self.has_projectile(inventory, content, false)? {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                    Outcome::CONSUME
                } else {
                    Outcome::Fail
                }
            }
            "CrossbowItem" => {
                if item
                    .component("minecraft:charged_projectiles")
                    .and_then(Value::as_array)
                    .is_some_and(|items| !items.is_empty())
                {
                    Outcome::CONSUME
                } else if self.has_projectile(inventory, content, true)? {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                    Outcome::CONSUME
                } else {
                    Outcome::Fail
                }
            }
            "TridentItem" => {
                if next_damage_breaks(item)
                    || self.trident_spin_strength(item, content)? > 0.
                        && !self.in_water_or_rain(world, content, entities, tick)?
                {
                    Outcome::Fail
                } else {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                    Outcome::CONSUME
                }
            }
            "SpyglassItem" => {
                self.play_use_local_sound(world, "minecraft:item.spyglass.use", 1., 1.)?;
                self.start_using(Some(item), hand, content, inventory, tick)?;
                Outcome::CONSUME
            }
            "BundleItem" => {
                self.start_using(Some(item), hand, content, inventory, tick)?;
                Outcome::CLIENT
            }
            "InstrumentItem" => {
                if let Some(instrument) = lifetime::instrument(item, content)? {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                    let sound = instrument
                        .get("sound_event")
                        .ok_or("missing instrument sound event")?;
                    let event = lifetime::sound_event(sound)?;
                    let volume = instrument
                        .get("range")
                        .and_then(Value::as_f64)
                        .ok_or("invalid instrument range")? as f32
                        / 16.;
                    self.play_use_entity_sound(
                        world,
                        event,
                        crate::audio::SoundCategory::Records,
                        Handle {
                            network_id: self.entity_id,
                            generation: self.sound_entity_generation()?,
                        },
                        self.position,
                        volume,
                        1.,
                        true,
                    )?;
                    let group = item
                        .component("minecraft:use_cooldown")
                        .and_then(|cooldown| cooldown.get("cooldown_group"))
                        .and_then(Value::as_str)
                        .unwrap_or(item_name(Some(item), content)?);
                    self.cooldowns.insert(
                        group.to_owned(),
                        tick.saturating_add_signed(i64::from(duration)),
                    );
                    Outcome::CONSUME
                } else {
                    Outcome::Fail
                }
            }
            "FoodOnAStickItem" => Outcome::Pass,
            "EmptyMapItem" => Outcome::CLIENT,
            "WritableBookItem" | "WrittenBookItem" => {
                self.queue_use_ui(UiTrigger::Book { hand })?;
                Outcome::CLIENT
            }
            "SnowballItem"
            | "EggItem"
            | "EnderpearlItem"
            | "ExperienceBottleItem"
            | "WindChargeItem"
            | "SplashPotionItem"
            | "LingeringPotionItem" => {
                let (event, category) = match short_class(content.item_use_owner(item.id, false)?) {
                    "SnowballItem" => (
                        "minecraft:entity.snowball.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    "EggItem" => (
                        "minecraft:entity.egg.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    "EnderpearlItem" => (
                        "minecraft:entity.ender_pearl.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    "ExperienceBottleItem" => (
                        "minecraft:entity.experience_bottle.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    "WindChargeItem" => (
                        "minecraft:entity.wind_charge.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    "SplashPotionItem" => (
                        "minecraft:entity.splash_potion.throw",
                        crate::audio::SoundCategory::Players,
                    ),
                    "LingeringPotionItem" => (
                        "minecraft:entity.lingering_potion.throw",
                        crate::audio::SoundCategory::Neutral,
                    ),
                    _ => unreachable!("matched projectile owner"),
                };
                let pitch = 0.4 / (world.next_float()? * 0.4 + 0.8);
                self.play_use_level_sound(
                    world,
                    event,
                    category,
                    self.position,
                    0.5,
                    pitch,
                    false,
                )?;
                self.consume_held(hand, inventory, 1)?;
                Outcome::CLIENT
            }
            "FireworkRocketItem" => {
                if self.gliding {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "FishingRodItem" => {
                let retrieving = entities.has_fishing_hook(self.entity_id)?;
                let pitch = 0.4 / (world.next_float()? * 0.4 + 0.8);
                self.play_use_level_sound(
                    world,
                    if retrieving {
                        "minecraft:entity.fishing_bobber.retrieve"
                    } else {
                        "minecraft:entity.fishing_bobber.throw"
                    },
                    crate::audio::SoundCategory::Neutral,
                    self.position,
                    if retrieving { 1. } else { 0.5 },
                    pitch,
                    false,
                )?;
                Outcome::CLIENT
            }
            "EnderEyeItem" => {
                let hit = self.item_raycast(world, content, false, false)?;
                if hit.is_some_and(|hit| {
                    content.name(hit.state) == Some("minecraft:end_portal_frame")
                }) {
                    Outcome::Pass
                } else {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                    Outcome::CONSUME
                }
            }
            "KnowledgeBookItem" => {
                self.consume_held(hand, inventory, 1)?;
                if item
                    .component("minecraft:recipes")
                    .and_then(Value::as_array)
                    .is_some_and(|recipes| !recipes.is_empty())
                {
                    Outcome::CLIENT
                } else {
                    Outcome::Fail
                }
            }
            "BucketItem" => self.bucket_result(hand, world, content, inventory, entities, tick)?,
            "BottleItem" => self.bottle_result(hand, world, content, inventory, entities, tick)?,
            "BoatItem" => self.boat_result(hand, world, content, inventory, entities, tick)?,
            "SpawnEggItem" => {
                self.spawn_egg_air_result(hand, world, content, inventory, entities, tick)?
            }
            "PlaceOnWaterBlockItem" => {
                self.water_block_result(hand, world, content, inventory, entities, tick)?
            }
            owner => return Err(format!("unknown verified air use owner {owner}")),
        };
        let _ = target;
        if result.consumes() && duration == 0 {
            let slot = if hand == 1 {
                45
            } else {
                36 + u16::from(inventory.selected_slot)
            };
            let transformed = self
                .use_predictions
                .iter()
                .skip(prediction_start)
                .rev()
                .find_map(|prediction| match prediction {
                    UsePrediction::PlayerSlot {
                        slot: candidate,
                        stack,
                        ..
                    } if *candidate == slot => Some(stack.clone()),
                    _ => None,
                });
            let transformed = transformed
                .unwrap_or_else(|| Some(item.clone()))
                .unwrap_or_else(|| {
                    let mut empty = item.clone();
                    empty.count = 0;
                    empty
                });
            self.after_item_use(item, transformed, hand, inventory, content, tick)?;
        }
        Ok(result)
    }

    pub(super) fn generic_item_use(
        &mut self,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        _entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(item) =
            super::super::interaction::held(inventory, hand).filter(|item| !item.empty())
        else {
            return Ok(Outcome::Pass);
        };
        if let Some(consumable) = item.component("minecraft:consumable") {
            if item.component("minecraft:food").is_some_and(|food| {
                !self.abilities.invulnerable
                    && self.food.is_none_or(|level| level >= 20)
                    && !food
                        .get("can_always_eat")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            }) {
                return Ok(Outcome::Fail);
            }
            let ticks = (consumable
                .get("consume_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(1.6) as f32
                * 20.) as i32;
            if ticks > 0 {
                self.start_using(Some(item), hand, content, inventory, tick)?;
            } else {
                let result = self.consume_item(item, world)?;
                self.predict_held(hand, (!result.empty()).then_some(result), inventory, false)?;
            }
            Ok(Outcome::CONSUME)
        } else if let Some(equippable) = item.component("minecraft:equippable").filter(|value| {
            value
                .get("swappable")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        }) {
            self.swap_equipment(item, equippable, hand, inventory, content)
        } else if item.component("minecraft:blocks_attacks").is_some() {
            self.start_using(Some(item), hand, content, inventory, tick)?;
            Ok(Outcome::CONSUME)
        } else if let Some(kinetic) = item.component("minecraft:kinetic_weapon") {
            self.start_using(Some(item), hand, content, inventory, tick)?;
            if let Some(sound) = kinetic.get("sound") {
                self.play_use_level_sound(
                    world,
                    lifetime::sound_event(sound)?,
                    crate::audio::SoundCategory::Players,
                    self.position,
                    1.,
                    1.,
                    true,
                )?;
            }
            Ok(Outcome::CONSUME)
        } else {
            Ok(Outcome::Pass)
        }
    }

    fn swap_equipment(
        &mut self,
        item: &Stack,
        equippable: &Value,
        hand: u8,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<Outcome> {
        if let Some(allowed) = equippable.get("allowed_entities") {
            let player = content
                .registry_entries("minecraft:entity_type")?
                .iter()
                .position(|(name, _)| name == "minecraft:player")
                .ok_or("missing player entity registry")? as i32;
            let allowed =
                if let Some(tag) = allowed.as_str().and_then(|value| value.strip_prefix('#')) {
                    content
                        .registry_tag("minecraft:entity_type", tag)?
                        .contains(&player)
                } else if let Some(name) = allowed.as_str() {
                    name == "minecraft:player"
                } else {
                    allowed
                        .as_array()
                        .ok_or("invalid equipment allowed entities")?
                        .iter()
                        .any(|name| name.as_str() == Some("minecraft:player"))
                };
            if !allowed {
                return Ok(Outcome::Pass);
            }
        }
        let slot = match equippable
            .get("slot")
            .and_then(Value::as_str)
            .ok_or("invalid equipment slot")?
        {
            "mainhand" => 36 + u16::from(inventory.selected_slot),
            "offhand" => 45,
            "feet" => 8,
            "legs" => 7,
            "chest" => 6,
            "head" => 5,
            "body" | "saddle" => return Ok(Outcome::Pass),
            _ => return Err("invalid equipment slot".into()),
        };
        let equipped = inventory
            .player
            .slots
            .get(usize::from(slot))
            .ok_or("equipment slot outside inventory")?
            .as_ref()
            .filter(|item| !item.empty());
        if equipped.is_some_and(|equipped| item.same_item(equipped))
            || self.game_mode != 1
                && enchantment_has(equipped, "minecraft:prevent_armor_change", content)?
        {
            return Ok(Outcome::Fail);
        }
        let mut equipment = item.clone();
        if item.count <= 1 {
            self.predict_use(UsePrediction::PlayerSlot {
                slot,
                stack: Some(equipment),
                replacement: true,
            })?;
            // Empty equipment returns the original object after copyAndClear in survival.
            self.predict_held(
                hand,
                equipped
                    .cloned()
                    .or_else(|| self.abilities.instabuild.then(|| item.clone())),
                inventory,
                equipped.is_some(),
            )?;
        } else {
            equipment.count = 1;
            self.predict_use(UsePrediction::PlayerSlot {
                slot,
                stack: Some(equipment),
                replacement: true,
            })?;
            self.consume_held(hand, inventory, 1)?;
            if let Some(equipped) = equipped {
                self.predict_use(UsePrediction::ExtraItem {
                    stack: equipped.clone(),
                    drop_remainder: true,
                })?;
            }
        }
        Ok(Outcome::CLIENT)
    }
}

#[cfg(test)]
mod p1_helmet_tests {
    use super::*;
    use crate::protocol::{
        world::{Chunk, Palette, Section, SurfaceHeightmap},
        Reader,
    };
    use serde_json::json;

    fn helmet(swappable: Option<bool>) -> Stack {
        let mut equippable = json!({"slot": "head"});
        if let Some(swappable) = swappable {
            equippable["swappable"] = json!(swappable);
        }
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:equippable".into(),
            equippable,
        )]));
        stack
    }

    fn content() -> Content {
        let mut content = Content::test_cube();
        content
            .item_names
            .insert(1, "minecraft:golden_helmet".into());
        content.insert_test_item_interaction(1, "Item", "Item", "Item");
        content
    }

    fn predicted_slot(player: &Player, slot: u16) -> Option<&Option<Stack>> {
        player
            .use_predictions
            .iter()
            .rev()
            .find_map(|prediction| match prediction {
                UsePrediction::PlayerSlot {
                    slot: candidate,
                    stack,
                    ..
                } if *candidate == slot => Some(stack),
                _ => None,
            })
    }

    fn air_world() -> World {
        let mut world = World::new(0, 16);
        let uniform = |size| Palette {
            bits: 0,
            values: vec![0],
            words: Vec::new(),
            size,
        };
        world
            .insert(
                [0, 0],
                Chunk {
                    sections: vec![Arc::new(Section {
                        blocks: uniform(4096),
                        biomes: uniform(64),
                        non_empty_block_count: 0,
                    })],
                    sky: BTreeMap::new(),
                    block: BTreeMap::new(),
                    block_entities: crate::protocol::world::empty_block_entities(),
                    surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        world
    }

    // Item.use and Equippable.swapWithEquipmentSlot: local 26.1.2 javap.
    // These are client predictions, never evidence of server equipment changes.
    #[test]
    fn p1_helmet_air_dispatch_respects_default_and_explicit_swappable() {
        let content = content();
        let world = World::new(0, 16);
        let entities = EntityStore::new();
        for (swappable, expected) in [
            (None, Outcome::CLIENT),
            (Some(true), Outcome::CLIENT),
            (Some(false), Outcome::Pass),
        ] {
            let mut inventory = InventoryStore::default();
            inventory.selected_slot = 1;
            inventory.player.slots[37] = Some(helmet(swappable));
            let mut player = Player::new();
            assert_eq!(
                player
                    .air_result(0, None, &world, &content, &inventory, &entities, 0)
                    .unwrap(),
                expected,
            );
            assert_eq!(
                predicted_slot(&player, 5).is_some(),
                expected == Outcome::CLIENT,
            );
            assert!(player.using.is_none());
            assert!(inventory.player.slots[5].is_none());
            assert_eq!(inventory.player.slots[37].as_ref().unwrap().count, 1);
        }
    }

    #[test]
    fn p1_helmet_air_prediction_equips_and_returns_previous_stack_in_both_hands() {
        let content = content();
        let world = World::new(0, 16);
        let entities = EntityStore::new();
        for hand in [0, 1] {
            for creative in [false, true] {
                for occupied in [false, true] {
                    let held = helmet(None);
                    let mut previous = Stack::default();
                    previous.id = 2;
                    previous.count = 1;
                    let mut inventory = InventoryStore::default();
                    inventory.selected_slot = 1;
                    let source = if hand == 0 { 37 } else { 45 };
                    inventory.player.slots[source] = Some(held.clone());
                    inventory.player.slots[5] = occupied.then(|| previous.clone());
                    let mut player = Player::new();
                    if creative {
                        player.set_mode(1).unwrap();
                    }
                    assert_eq!(
                        player
                            .air_result(hand, None, &world, &content, &inventory, &entities, 0)
                            .unwrap(),
                        Outcome::CLIENT,
                    );
                    assert_eq!(predicted_slot(&player, 5), Some(&Some(held.clone())));
                    let returned = if occupied {
                        Some(previous.clone())
                    } else {
                        creative.then(|| held.clone())
                    };
                    assert_eq!(predicted_slot(&player, source as u16), Some(&returned));
                    assert_eq!(inventory.player.slots[source], Some(held));
                    assert_eq!(
                        inventory.player.slots[5],
                        occupied.then(|| previous.clone()),
                    );
                    assert!(!player.use_predictions.iter().any(|prediction| {
                        matches!(prediction, UsePrediction::ExtraItem { .. })
                    }));
                }
            }
        }
    }

    #[test]
    fn p1_helmet_air_ordinary_use_emits_hand_sequence_rotation_and_swing_not_click() {
        let content = content();
        let world = air_world();
        let mut entities = EntityStore::new();
        entities.activate_models(Arc::new(crate::entities::Models {
            rigs: BTreeMap::new(),
            schemas: BTreeMap::new(),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            armor_models: BTreeMap::new(),
            motion: BTreeMap::new(),
        }));
        for hand in [0, 1] {
            let mut inventory = InventoryStore::default();
            inventory.selected_slot = 1;
            inventory.player.slots[if hand == 0 { 37 } else { 45 }] = Some(helmet(None));
            let mut player = Player::new();
            player.position = [8., 1., 8.];
            player.pose_known = true;
            player.yaw = 12.;
            player.pitch = -5.;
            let context = InteractionContext {
                pose_revision: player.pose_revision,
                inventory_revision: inventory.revision,
                target: None,
            };
            let mut packets = Vec::new();
            player
                .execute(
                    InteractionIntent::Use { hand },
                    &context,
                    &world,
                    &content,
                    &inventory,
                    &entities,
                    0,
                    &mut packets,
                )
                .unwrap();
            assert_eq!(
                packets.iter().map(|packet| packet.id).collect::<Vec<_>>(),
                vec![67, 63],
            );
            let mut use_item = Reader::new(&packets[0].payload.0);
            assert_eq!(use_item.var().unwrap(), i32::from(hand));
            assert_eq!(use_item.var().unwrap(), 1);
            assert_eq!(use_item.f32().unwrap(), 12.);
            assert_eq!(use_item.f32().unwrap(), -5.);
            assert_eq!(use_item.offset, use_item.bytes.len());
            let mut swing = Reader::new(&packets[1].payload.0);
            assert_eq!(swing.var().unwrap(), i32::from(hand));
            assert_eq!(swing.offset, swing.bytes.len());
            assert_eq!(predicted_slot(&player, 5), Some(&Some(helmet(None))));
            assert_eq!(
                player.last_interaction_result,
                Some(Outcome::CLIENT.java_result())
            );
        }
    }
}
