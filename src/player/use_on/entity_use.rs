//! Client-side entity interaction branches of the pinned 26.1.2 declaring classes.
//! The packet carries one relative hit location in this version: there is no second
//! interactAt call. Entity.interact runs before ItemStack.interactLivingEntity.
use super::*;
use crate::entities::{Entity, Metadata, Models};

/// Unsynced client fields used by the actual declaring methods. These are not
/// snapshots of server NBT; their lifetime is the entity's generational handle.
#[derive(Clone, Debug, Default)]
pub(crate) struct EntityUseState {
    pub age_lock_particle_timer: i32,
    pub forced_age_timer: i32,
    pub tadpole_age: i32,
    pub furnace_fuel: i32,
    pub furnace_push: [f64; 3],
    pub mooshroom_stew: Option<Value>,
    pub ordered_to_sit: bool,
    pub in_love: i32,
    pub stand_counter: i32,
    pub allay_duplication_cooldown: i64,
    pub allay_liked_player: Option<String>,
}

struct Target<'a> {
    entity: &'a Entity,
    models: &'a Models,
    store: &'a EntityStore,
}
impl<'a> Target<'a> {
    fn new(store: &'a EntityStore, handle: Handle) -> Result<Self> {
        let entity = store.resolve(handle)?;
        // Dragon part handles resolve to their parent for presentation, but their
        // interaction method is Entity's PASS, not the parent's living method.
        let models = store.models_ref().ok_or("entity models not activated")?;
        if !models.schemas.contains_key(&entity.type_name) {
            return Err("entity schema missing".into());
        }
        Ok(Self {
            entity,
            models,
            store,
        })
    }
    fn is(&self, class: &str) -> bool {
        self.models.schemas[&self.entity.type_name]
            .hierarchy
            .iter()
            .any(|name| short_class(name) == class)
    }
    fn owner(&self, method: &str) -> Result<&str> {
        self.models.schemas[&self.entity.type_name]
            .method_owners
            .get(method)
            .map(|owner| short_class(owner))
            .ok_or_else(|| format!("missing Source entity method owner {method}"))
    }
    fn metadata(&self, key: &str) -> Result<Option<&Metadata>> {
        self.store.metadata(self.entity.handle, key)
    }
    fn number(&self, key: &str, source_default: i32) -> Result<i32> {
        self.metadata(key)?.map_or(Ok(source_default), |value| {
            value
                .int()
                .ok_or_else(|| format!("invalid Source integer accessor {key}"))
        })
    }
    fn boolean(&self, key: &str, source_default: bool) -> Result<bool> {
        self.metadata(key)?.map_or(Ok(source_default), |value| {
            value
                .boolean()
                .ok_or_else(|| format!("invalid Source boolean accessor {key}"))
        })
    }
    fn health(&self) -> Result<f32> {
        match self.metadata("DATA_HEALTH_ID")? {
            Some(Metadata::Float(value)) => Ok(*value),
            None => self.max_health(),
            _ => Err("invalid Source health accessor".into()),
        }
    }
    fn max_health(&self) -> Result<f32> {
        Ok(self
            .store
            .attribute(self.entity.handle, "minecraft:max_health")? as f32)
    }
    fn alive(&self) -> Result<bool> {
        Ok(self.entity.death_tick.is_none() && (!self.is("LivingEntity") || self.health()? > 0.))
    }
    fn baby(&self) -> Result<bool> {
        // These two undead variants override isBaby to false, even though their
        // superclass declares a baby accessor.
        if self.is("CamelHusk") || self.is("ZombieNautilus") {
            return Ok(false);
        }
        self.boolean("DATA_BABY_ID", false)
    }
    fn age_locked(&self) -> Result<bool> {
        self.boolean("AGE_LOCKED", false)
    }
    fn tame(&self) -> Result<bool> {
        if self.is("AbstractHorse") {
            Ok(self.number("DATA_ID_FLAGS", if self.is("Camel") { 2 } else { 0 })? & 2 != 0)
        } else {
            Ok(self.number("DATA_FLAGS_ID", 0)? & 4 != 0)
        }
    }
    fn owned(&self, player: &Player) -> Result<bool> {
        let Some(Metadata::EntityReference(Some(uuid))) = self.metadata("DATA_OWNERUUID_ID")?
        else {
            return Ok(false);
        };
        Ok(player
            .uuid
            .as_deref()
            .is_some_and(|identity| identity == uuid))
    }
    fn silent(&self) -> Result<bool> {
        self.boolean("DATA_SILENT", false)
    }
    fn equipment(&self, slot: u8) -> Option<&Stack> {
        self.entity
            .equipment_value(usize::from(slot))
            .filter(|stack| !stack.empty())
    }
    fn slot_usable(&self, slot: u8) -> Result<bool> {
        if self.is("Horse")
            || self.is("SkeletonHorse")
            || self.is("ZombieHorse")
            || self.is("Llama")
        {
            return Ok(true);
        }
        if self.is("AbstractNautilus") && matches!(slot, 6 | 7) {
            return Ok(self.alive()? && !self.baby()? && self.tame()?);
        }
        if self.is("AbstractHorse") && slot == 7 {
            return Ok(self.alive()? && !self.baby()? && self.tame()?);
        }
        if (self.is("Pig") || self.is("Strider")) && slot == 7 || self.is("HappyGhast") && slot == 6
        {
            return Ok(self.alive()? && !self.baby()?);
        }
        Ok(true)
    }
    fn equippable(&self, stack: &Stack, slot: u8, content: &Content) -> Result<bool> {
        let Some(component) = stack.component("minecraft:equippable") else {
            return Ok(slot == 0 && self.slot_usable(0)?);
        };
        if equipment_slot(
            component
                .get("slot")
                .and_then(Value::as_str)
                .ok_or("equippable slot missing")?,
        )? != slot
            || !self.slot_usable(slot)?
        {
            return Ok(false);
        }
        match component.get("allowed_entities") {
            None => Ok(true),
            Some(holders) => entity_holders(holders, self.entity, content),
        }
    }
    fn wearing(&self, slot: u8, content: &Content) -> Result<bool> {
        self.equipment(slot)
            .map_or(Ok(false), |item| self.equippable(item, slot, content))
    }
    fn food(&self, item: Option<&Stack>, content: &Content) -> Result<bool> {
        // Tag declarations were read from every actual isFood owner, including
        // inherited AbstractCow/AbstractHorse and undead variant overrides.
        let tag = self.models.schemas[&self.entity.type_name]
            .hierarchy
            .iter()
            .find_map(|class| match short_class(class) {
                "Armadillo" => Some("minecraft:armadillo_food"),
                "Axolotl" => Some("minecraft:axolotl_food"),
                "Bee" => Some("minecraft:bee_food"),
                "CamelHusk" => Some("minecraft:camel_husk_food"),
                "Camel" => Some("minecraft:camel_food"),
                "Chicken" => Some("minecraft:chicken_food"),
                "AbstractCow" => Some("minecraft:cow_food"),
                "Llama" => Some("minecraft:llama_food"),
                "ZombieHorse" => Some("minecraft:zombie_horse_food"),
                "AbstractHorse" => Some("minecraft:horse_food"),
                "Cat" => Some("minecraft:cat_food"),
                "Ocelot" => Some("minecraft:ocelot_food"),
                "Fox" => Some("minecraft:fox_food"),
                "Frog" | "Tadpole" => Some("minecraft:frog_food"),
                "Goat" => Some("minecraft:goat_food"),
                "HappyGhast" => Some("minecraft:happy_ghast_food"),
                "Panda" => Some("minecraft:panda_food"),
                "Pig" => Some("minecraft:pig_food"),
                "Rabbit" => Some("minecraft:rabbit_food"),
                "Sheep" => Some("minecraft:sheep_food"),
                "Sniffer" => Some("minecraft:sniffer_food"),
                "Turtle" => Some("minecraft:turtle_food"),
                "Wolf" => Some("minecraft:wolf_food"),
                "Strider" => Some("minecraft:strider_food"),
                "Hoglin" => Some("minecraft:hoglin_food"),
                _ => None,
            });
        if self.is("AbstractNautilus") {
            return item_tag(
                item,
                content,
                if self.tame()? || self.baby()? {
                    "minecraft:nautilus_food"
                } else {
                    "minecraft:nautilus_taming_items"
                },
            );
        }
        match tag {
            Some(tag) => item_tag(item, content, tag),
            None => Ok(false),
        }
    }
}

fn equipment_slot(slot: &str) -> Result<u8> {
    match slot {
        "mainhand" => Ok(0),
        "offhand" => Ok(1),
        "feet" => Ok(2),
        "legs" => Ok(3),
        "chest" => Ok(4),
        "head" => Ok(5),
        "body" => Ok(6),
        "saddle" => Ok(7),
        _ => Err("unknown Source equipment slot".into()),
    }
}
fn entity_holders(holders: &Value, entity: &Entity, content: &Content) -> Result<bool> {
    if let Some(name) = holders.as_str() {
        if let Some(tag) = name.strip_prefix('#') {
            return content.registry_tag_contains("minecraft:entity_type", tag, entity.kind as i32);
        }
        return Ok(name == entity.type_name);
    }
    let holders = holders.as_array().ok_or("invalid entity holder set")?;
    for holder in holders {
        let name = holder.as_str().ok_or("invalid entity holder")?;
        if name == entity.type_name {
            return Ok(true);
        }
    }
    Ok(false)
}
fn dye(item: Option<&Stack>) -> Result<Option<i32>> {
    let Some(color) = item.and_then(|item| item.component("minecraft:dye")) else {
        return Ok(None);
    };
    let name = color.as_str().ok_or("invalid dye component")?;
    DYE_COLORS
        .iter()
        .position(|color| *color == name)
        .map(|id| Some(id as i32))
        .ok_or_else(|| "unknown Source dye color".into())
}

impl Player {
    pub(in crate::player) fn entity_result(
        &mut self,
        handle: Handle,
        point: [f64; 3],
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let target = Target::new(entities, handle)?;
        if target.entity.handle != handle || self.game_mode == 3 {
            return Ok(Outcome::Pass);
        }
        let item = super::super::interaction::held(inventory, hand).filter(|item| !item.empty());
        let result = match target.owner("interact")? {
            "Interaction" => {
                if target.boolean("DATA_RESPONSE_ID", false)? {
                    Outcome::CLIENT
                } else {
                    Outcome::CONSUME
                }
            }
            "LeashFenceKnotEntity" => Outcome::CLIENT,
            // All armor-stand swap/disabled-slot logic is behind the server guard.
            "ArmorStand" => {
                if target.number("DATA_CLIENT_FLAGS", 0)? & 16 != 0
                    || item_name(item, content)? == "minecraft:name_tag"
                {
                    self.base_entity_interaction(
                        &target, item, hand, world, content, inventory, tick,
                    )?
                } else {
                    Outcome::CONSUME
                }
            }
            "ItemFrame" => {
                // Fixed is unsynced and defaults false on the client. Item/rotation
                // writes and map tracked-count validation are server-only.
                let filled = matches!(target.metadata("DATA_ITEM")?, Some(Metadata::Item(stack)) if !stack.empty());
                if filled || item.is_some() {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "Minecart" => {
                if self.last_sent_input & 32 == 0 && target.entity.passenger_ids.is_empty() {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "MinecartFurnace" => {
                if item_tag(item, content, "minecraft:furnace_minecart_fuel")? {
                    let mut state = self.entity_use_state(&target)?;
                    if state.furnace_fuel.wrapping_add(3600) <= 32000 {
                        state.furnace_fuel = state.furnace_fuel.wrapping_add(3600);
                        if state.furnace_fuel > 0 {
                            state.furnace_push = [
                                target.entity.position[0] - self.position[0],
                                0.,
                                target.entity.position[2] - self.position[2],
                            ];
                        }
                        self.predict_entity_state(&target, state)?;
                        self.consume_held(hand, inventory, 1)?;
                    }
                }
                Outcome::CLIENT
            }
            "MinecartCommandBlock" => {
                if self.game_mode == 1 && self.permission_level.is_some_and(|level| level >= 2) {
                    self.queue_use_ui(UiTrigger::MinecartCommand { handle })?;
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "AbstractMinecartContainer" | "MinecartChest" => Outcome::CLIENT,
            "AbstractBoat" | "AbstractChestBoat" => {
                let base = self.base_entity_interaction(
                    &target, item, hand, world, content, inventory, tick,
                )?;
                if base != Outcome::Pass {
                    base
                } else if self.last_sent_input & 32 == 0
                    && target.store.boat_out_of_control_ticks(handle)? < 60.
                {
                    Outcome::CLIENT
                } else if target.is("AbstractChestBoat") {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "Mob" | "CamelHusk" | "ZombieHorse" | "AbstractNautilus" => {
                if !target.alive()? {
                    Outcome::Pass
                } else {
                    let important = self.important_entity_interaction(
                        &target, item, hand, world, content, inventory,
                    )?;
                    if important.consumes() {
                        important
                    } else {
                        let base = self.base_entity_interaction(
                            &target, item, hand, world, content, inventory, tick,
                        )?;
                        if base != Outcome::Pass {
                            base
                        } else {
                            self.mob_entity_interaction(
                                &target, item, hand, world, content, inventory, tick,
                            )?
                        }
                    }
                }
            }
            "Entity" => {
                self.base_entity_interaction(&target, item, hand, world, content, inventory, tick)?
            }
            owner => return Err(format!("unknown Source entity interaction owner {owner}")),
        };
        if result.consumes() {
            return Ok(result);
        }
        // Player.interactOn intentionally ignores non-consuming entity FAIL and
        // invokes the item against a living target. A copied creative stack is
        // used here, whereas entity methods receive the actual hand reference.
        if target.is("LivingEntity") {
            self.living_item_interaction(&target, item, hand, world, content, inventory)
        } else {
            let _ = point; // Armor-stand location is used only behind its server guard.
            Ok(Outcome::Pass)
        }
    }

    fn important_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
    ) -> Result<Outcome> {
        if item_name(item, content)? == "minecraft:name_tag" {
            let result =
                self.living_item_interaction(target, item, hand, world, content, inventory)?;
            if result.consumes() {
                return Ok(result);
            }
        }
        if let Some(stack) = item {
            if short_class(content.item_class(stack.id)?) == "SpawnEggItem" {
                return Ok(Outcome::CONSUME);
            }
        }
        Ok(Outcome::Pass)
    }

    fn living_item_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        _hand: u8,
        world: &World,
        content: &Content,
        _inventory: &InventoryStore,
    ) -> Result<Outcome> {
        let Some(item) = item else {
            return Ok(Outcome::Pass);
        };
        if let Some(equippable) = item.component("minecraft:equippable") {
            if equippable
                .get("equip_on_interact")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let slot = equipment_slot(
                    equippable
                        .get("slot")
                        .and_then(Value::as_str)
                        .ok_or("equippable slot missing")?,
                )?;
                if target.equippable(item, slot, content)?
                    && target.equipment(slot).is_none()
                    && target.alive()?
                {
                    // Equippable.equipOnTarget splits/equips only on the server.
                    return Ok(Outcome::CLIENT);
                }
            }
        }
        match short_class(content.item_class(item.id)?) {
            "NameTagItem" => Ok(
                if item.component("minecraft:custom_name").is_some()
                    && content.entity_can_serialize(&target.entity.type_name)?
                {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                },
            ),
            "DyeItem" => {
                if target.is("Sheep")
                    && target.alive()?
                    && target.number("DATA_WOOL_ID", 0)? & 16 == 0
                {
                    let current_color = target.number("DATA_WOOL_ID", 0)? & 15;
                    if dye(Some(item))?.is_some_and(|color| color != current_color) {
                        self.play_use_entity_sound(
                            world,
                            "minecraft:item.dye.use",
                            crate::audio::SoundCategory::Players,
                            target.entity.handle,
                            target.entity.position,
                            1.,
                            1.,
                            true,
                        )?;
                        return Ok(Outcome::CLIENT);
                    }
                }
                Ok(Outcome::Pass)
            }
            // The complete declaring cohort is Item, DyeItem and NameTagItem.
            // Every other actual item class inherits Item's default PASS.
            _ => Ok(Outcome::Pass),
        }
    }

    fn entity_use_state(&self, target: &Target<'_>) -> Result<EntityUseState> {
        for prediction in self.use_predictions.iter().rev() {
            if let UsePrediction::EntityUseState { handle, state } = prediction {
                if *handle == target.entity.handle {
                    return Ok(state.clone());
                }
            }
        }
        Ok(target.store.source_use_state(target.entity.handle)?.clone())
    }
    fn predict_entity_state(&mut self, target: &Target<'_>, state: EntityUseState) -> Result<()> {
        self.predict_use(UsePrediction::EntityUseState {
            handle: target.entity.handle,
            state,
        })
    }
    fn predict_entity_metadata(
        &mut self,
        target: &Target<'_>,
        key: &'static str,
        value: Metadata,
    ) -> Result<()> {
        self.predict_use(UsePrediction::EntityMetadata {
            handle: target.entity.handle,
            key,
            value,
        })
    }
    fn predict_entity_equipment(
        &mut self,
        target: &Target<'_>,
        slot: u8,
        stack: Option<Stack>,
    ) -> Result<()> {
        self.predict_use(UsePrediction::EntityEquipment {
            handle: target.entity.handle,
            slot,
            stack,
        })
    }
}

impl Player {
    fn base_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        _hand: u8,
        world: &World,
        content: &Content,
        _inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        let name = item_name(item, content)?;
        if name == "minecraft:shears" {
            let mut removed = false;
            let bounds = target.store.bounds(target.entity.handle, tick, 1.)?;
            let center: [f64; 3] =
                std::array::from_fn(|axis| (bounds.min[axis] + bounds.max[axis]) * 0.5);
            let area = super::super::Bounds {
                min: center.map(|coordinate| coordinate - 16.),
                max: center.map(|coordinate| coordinate + 16.),
            };
            for entity in target.store.candidates(area) {
                if entity.leash_value() == Some(target.entity.handle)
                    && target
                        .store
                        .bounds(entity.handle, tick, 1.)?
                        .intersects(area)
                {
                    self.predict_use(UsePrediction::EntityLeash {
                        handle: entity.handle,
                        holder: None,
                    })?;
                    removed = true;
                }
            }
            // dropLeash only drops a resolved connection, not a delayed numeric
            // holder reference whose entity has not arrived yet.
            if (target.is("Mob") || target.is("AbstractBoat"))
                && target.entity.leash_value().is_some()
            {
                self.predict_use(UsePrediction::EntityLeash {
                    handle: target.entity.handle,
                    holder: None,
                })?;
                removed = true;
            }
            if removed {
                // hurtAndBreak(LivingEntity, slot) is itself server-level guarded.
                return Ok(Outcome::CLIENT);
            }
            let may_shear = if target.is("Wolf") {
                target.owned(self)?
            } else {
                target.entity.passenger_ids.is_empty()
            };
            if target.is("Mob") && may_shear && self.last_sent_input & 32 == 0 {
                for slot in 0..8 {
                    let Some(stack) = target.equipment(slot) else {
                        continue;
                    };
                    let Some(equippable) = stack.component("minecraft:equippable") else {
                        continue;
                    };
                    if equippable
                        .get("can_be_sheared")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        && (self.game_mode == 1
                            || !enchantment_has(
                                Some(stack),
                                "minecraft:prevent_armor_change",
                                content,
                            )?)
                    {
                        self.predict_entity_equipment(target, slot, None)?;
                        // Entity.playSound excludes null rather than the local
                        // player: consume Level's seed, never invent a local voice.
                        if !target.silent()? {
                            let _ = world.sound_seed()?;
                        }
                        return Ok(Outcome::CLIENT);
                    }
                }
            }
        }
        if target.alive()? && (target.is("Mob") || target.is("AbstractBoat")) {
            let holder = target.entity.leash_value();
            if holder.is_some_and(|holder| holder.network_id == self.entity_id) {
                return Ok(Outcome::CLIENT_WITHOUT_ITEM);
            }
            if name == "minecraft:lead"
                && !holder
                    .map(|holder| {
                        Target::new(target.store, holder).map(|holder| holder.is("Player"))
                    })
                    .transpose()?
                    .unwrap_or(false)
            {
                return Ok(Outcome::CONSUME);
            }
        }
        Ok(Outcome::Pass)
    }

    fn animal_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        if target.is("Animal") && target.food(item, content)? {
            if target.baby()? && !target.age_locked()? {
                self.use_entity_food(item, hand, inventory, content)?;
                // getAge is -1 on the client; speed-up seconds is therefore 0,
                // and ageUp(0,true) preserves baby while starting its real timer.
                let mut state = self.entity_use_state(target)?;
                if state.forced_age_timer == 0 {
                    state.forced_age_timer = 40;
                }
                self.predict_entity_state(target, state)?;
                self.entity_eating(target, world, content)?;
                return Ok(Outcome::CLIENT);
            }
            return Ok(Outcome::CONSUME);
        }
        self.ageable_entity_interaction(target, item, hand, world, content, inventory, tick)
    }

    fn ageable_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        if (target.is("AgeableMob") || target.is("Tadpole"))
            && item_name(item, content)? == "minecraft:golden_dandelion"
            && (target.baby()? || target.is("Tadpole"))
            && self.entity_use_state(target)?.age_lock_particle_timer == 0
            && !content.registry_tag_contains(
                "minecraft:entity_type",
                "minecraft:cannot_be_age_locked",
                target.entity.kind as i32,
            )?
        {
            let locked = !target.age_locked()?;
            self.predict_entity_metadata(target, "AGE_LOCKED", Metadata::Bool(locked))?;
            let mut state = self.entity_use_state(target)?;
            state.age_lock_particle_timer = 40;
            if target.is("Tadpole") {
                state.tadpole_age = 0;
            }
            self.predict_entity_state(target, state)?;
            self.consume_held(hand, inventory, 1)?;
            self.play_use_level_sound(
                world,
                if locked {
                    "minecraft:item.golden_dandelion.use"
                } else {
                    "minecraft:item.golden_dandelion.unuse"
                },
                crate::audio::SoundCategory::Players,
                target.entity.position.map(f64::floor),
                1.,
                1.,
                false,
            )?;
            return Ok(Outcome::CLIENT);
        }
        Ok(Outcome::Pass)
    }

    fn use_entity_food(
        &mut self,
        item: Option<&Stack>,
        hand: u8,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<()> {
        let Some(before) = item else {
            return Ok(());
        };
        let Some(remainder) = before.component("minecraft:use_remainder") else {
            return self.consume_held(hand, inventory, 1);
        };
        // Mob.usePlayerItem saves this component before mutating the same stack.
        // With a remainder, setItemInHand runs even for a surviving same reference.
        self.consume_held(hand, inventory, 1)?;
        if !self.abilities.instabuild {
            let remainder = crate::inventory::template_stack(remainder, content)?;
            if before.count <= 1 {
                self.predict_held(hand, Some(remainder), inventory, true)?;
            } else {
                self.predict_use(UsePrediction::ExtraItem {
                    stack: remainder,
                    drop_remainder: true,
                })?;
            }
        }
        Ok(())
    }

    fn heal_entity(&mut self, target: &Target<'_>, amount: f32) -> Result<bool> {
        let before = target.health()?;
        if before <= 0. {
            return Ok(false);
        }
        let after = (before + amount).clamp(0., target.max_health()?);
        if after != before {
            self.predict_entity_metadata(target, "DATA_HEALTH_ID", Metadata::Float(after))?;
        }
        Ok(after != before)
    }
    fn feed_entity(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        inventory: &InventoryStore,
        world: &World,
        content: &Content,
        multiplier: f32,
        fallback: f32,
    ) -> Result<()> {
        let nutrition = item
            .and_then(|item| item.component("minecraft:food"))
            .and_then(|food| food.get("nutrition"))
            .and_then(Value::as_i64)
            .map(|nutrition| nutrition as f32);
        self.use_entity_food(item, hand, inventory, content)?;
        self.heal_entity(
            target,
            nutrition.map_or(fallback, |nutrition| multiplier * nutrition),
        )?;
        self.entity_eating(target, world, content)
    }
}

impl Player {
    fn mob_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        let name = item_name(item, content)?;
        let secondary = self.last_sent_input & 32 != 0;
        let owner = target.owner("mobInteract")?;
        match owner {
            "Mob" => Ok(Outcome::Pass),
            "AgeableMob" => {
                self.ageable_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Animal" | "Hoglin" => {
                self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "AbstractHorse"
            | "Horse"
            | "AbstractChestedHorse"
            | "SkeletonHorse"
            | "ZombieHorse"
            | "Camel" => {
                self.horse_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "AbstractNautilus" => {
                if target.baby()? {
                    return self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    );
                }
                if target.tame()? && secondary {
                    // openCustomInventoryScreen sends only from the server.
                    return Ok(Outcome::CLIENT);
                }
                if target.food(item, content)? && target.health()? < target.max_health()? {
                    self.feed_entity(target, item, hand, inventory, world, content, 2., 1.)?;
                    return Ok(Outcome::CLIENT);
                }
                let result =
                    self.living_item_interaction(target, item, hand, world, content, inventory)?;
                if result.consumes() {
                    return Ok(result);
                }
                if target.tame()? && !secondary && !target.food(item, content)? {
                    Ok(Outcome::CLIENT)
                } else {
                    self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )
                }
            }
            "AbstractFish" | "Axolotl" => {
                // Optional.orElse eagerly evaluates the superclass, but a water
                // bucket is neither food nor a golden dandelion in Source.
                if let Some(result) = self.bucket_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )? {
                    return Ok(result);
                }
                self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Tadpole" => {
                if target.food(item, content)? && !target.age_locked()? {
                    self.use_entity_food(item, hand, inventory, content)?;
                    let mut state = self.entity_use_state(target)?;
                    let remaining = (24000 - state.tadpole_age).max(0);
                    state.tadpole_age = state
                        .tadpole_age
                        .wrapping_add(((remaining / 20) as f32 * 0.1) as i32 * 20);
                    self.predict_entity_state(target, state)?;
                    self.entity_happy_particle(target, tick)?;
                    return Ok(Outcome::CLIENT);
                }
                let result = self.ageable_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )?;
                if result.consumes() {
                    return Ok(result);
                }
                Ok(self
                    .bucket_entity_interaction(target, item, hand, world, content, inventory, tick)?
                    .unwrap_or(Outcome::Pass))
            }
            "AbstractCow" | "Goat" => {
                if name == "minecraft:bucket" && !target.baby()? {
                    let sound = if owner == "AbstractCow" {
                        "minecraft:entity.cow.milk"
                    } else if target.boolean("DATA_IS_SCREAMING_GOAT", false)? {
                        "minecraft:entity.goat.screaming.milk"
                    } else {
                        "minecraft:entity.goat.milk"
                    };
                    self.play_use_local_sound(world, sound, 1., 1.)?;
                    self.create_filled_result(
                        item.ok_or("missing milk bucket")?,
                        new_entity_item(content, "minecraft:milk_bucket")?,
                        hand,
                        inventory,
                        true,
                    )?;
                    return Ok(Outcome::CLIENT);
                }
                let result = self.animal_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )?;
                if owner == "Goat" && result.consumes() && target.food(item, content)? {
                    self.entity_eating(target, world, content)?;
                }
                Ok(result)
            }
            "MushroomCow" => self
                .mooshroom_entity_interaction(target, item, hand, world, content, inventory, tick),
            "Armadillo" => {
                if name == "minecraft:brush" && !target.baby()? {
                    // Scute loot, voice and hurtAndBreak(LivingEntity) are all
                    // ServerLevel guarded; the client only returns SUCCESS.
                    return Ok(Outcome::CLIENT);
                }
                if target.number("ARMADILLO_STATE", 0)? != 0 {
                    Ok(Outcome::Fail)
                } else {
                    self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )
                }
            }
            "Bee" => {
                if target.food(item, content)?
                    && item
                        .map(|item| content.bee_food_effect(item.id))
                        .transpose()?
                        .flatten()
                        .is_some()
                {
                    self.use_entity_food(item, hand, inventory, content)?;
                    return Ok(Outcome::CLIENT);
                }
                self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Dolphin" => {
                if item_tag(item, content, "minecraft:fishes")? {
                    // gotFish and the sound are server guarded. consume is not.
                    self.consume_held(hand, inventory, 1)?;
                    Ok(Outcome::CLIENT)
                } else {
                    Ok(Outcome::Pass)
                }
            }
            "Sheep" => {
                if name == "minecraft:shears" {
                    // This owner returns CONSUME even for babies/already shorn
                    // sheep; both readiness and mutations live in ServerLevel.
                    Ok(Outcome::CONSUME)
                } else {
                    self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )
                }
            }
            "SnowGolem" | "Bogged" => {
                let ready = if owner == "SnowGolem" {
                    target.number("DATA_PUMPKIN_ID", 16)? & 16 != 0
                } else {
                    !target.boolean("DATA_SHEARED", false)?
                };
                Ok(if name == "minecraft:shears" && ready && target.alive()? {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                })
            }
            "CopperGolem" => {
                if item.is_none() && target.equipment(0).is_some() {
                    // BehaviorUtils.throwItem returns before creating an item
                    // on ClientLevel, but the following equipment write runs.
                    self.predict_entity_equipment(target, 0, None)?;
                    return Ok(Outcome::CLIENT);
                }
                Ok(
                    if name == "minecraft:shears"
                        && target.alive()?
                        && item_tag(
                            target.equipment(7),
                            content,
                            "minecraft:shearable_from_copper_golem",
                        )?
                    {
                        Outcome::CLIENT
                    } else {
                        // Wax, scraping and oxidation are all after isClientSide.
                        Outcome::Pass
                    },
                )
            }
            "IronGolem" => {
                if name != "minecraft:iron_ingot" || !self.heal_entity(target, 25.)? {
                    return Ok(Outcome::Pass);
                }
                let pitch = self.entity_random_pitch(target)?;
                self.entity_null_sound(target, world, "minecraft:entity.iron_golem.repair", pitch)?;
                self.consume_held(hand, inventory, 1)?;
                Ok(Outcome::CLIENT)
            }
            "Pig" | "Strider" => {
                let food = target.food(item, content)?;
                if !food
                    && target.wearing(7, content)?
                    && target.entity.passenger_ids.is_empty()
                    && !secondary
                {
                    return Ok(Outcome::CLIENT);
                }
                let result = self.animal_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )?;
                if result.consumes() {
                    if owner == "Strider" && food {
                        let pitch = self.entity_random_pitch_if_audible(target)?;
                        if let Some(pitch) = pitch {
                            self.entity_null_sound(
                                target,
                                world,
                                "minecraft:entity.strider.eat",
                                pitch,
                            )?;
                        }
                    }
                    return Ok(result);
                }
                if item
                    .map(|item| target.equippable(item, 7, content))
                    .transpose()?
                    .unwrap_or(false)
                {
                    self.living_item_interaction(target, item, hand, world, content, inventory)
                } else {
                    Ok(Outcome::Pass)
                }
            }
            "HappyGhast" => {
                if !target.baby()? {
                    let result = self
                        .living_item_interaction(target, item, hand, world, content, inventory)?;
                    if result.consumes() {
                        return Ok(result);
                    }
                    if target.wearing(6, content)? && !secondary {
                        return Ok(Outcome::CLIENT);
                    }
                }
                self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Sniffer" => {
                let result = self.animal_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )?;
                if result.consumes() && target.food(item, content)? {
                    self.entity_eating(target, world, content)?;
                }
                Ok(result)
            }
            "Cat" => {
                if target.tame()? && target.owned(self)? {
                    if item_tag(item, content, "minecraft:cat_collar_dyes")? {
                        let current_color = target.number("DATA_COLLAR_COLOR", 14)?;
                        if dye(item)?.is_some_and(|color| color != current_color) {
                            return Ok(Outcome::CLIENT);
                        }
                    } else if target.food(item, content)?
                        && target.health()? < target.max_health()?
                    {
                        return Ok(Outcome::CLIENT);
                    }
                    let result = self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )?;
                    if result.consumes() {
                        return Ok(result);
                    }
                    let mut state = self.entity_use_state(target)?;
                    state.ordered_to_sit = !state.ordered_to_sit;
                    self.predict_entity_state(target, state)?;
                    return Ok(Outcome::CLIENT);
                }
                if !target.tame()? && target.food(item, content)? {
                    return Ok(Outcome::CLIENT);
                }
                self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Wolf" => {
                self.wolf_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Ocelot" => {
                let distance = (0..3)
                    .map(|axis| (self.position[axis] - target.entity.position[axis]).powi(2))
                    .sum::<f64>();
                // registerGoals constructs TemptGoal only in Mob's server AI
                // branch. Its client field is null, never server goal state.
                if !target.boolean("DATA_TRUSTING", false)?
                    && target.food(item, content)?
                    && distance < 9.
                {
                    self.use_entity_food(item, hand, inventory, content)?;
                    Ok(Outcome::CLIENT)
                } else {
                    self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )
                }
            }
            "Parrot" => {
                if !target.tame()? && item_tag(item, content, "minecraft:parrot_food")? {
                    self.use_entity_food(item, hand, inventory, content)?;
                    if let Some(pitch) = self.entity_random_pitch_if_audible(target)? {
                        self.entity_null_sound(
                            target,
                            world,
                            "minecraft:entity.parrot.eat",
                            pitch,
                        )?;
                    }
                    return Ok(Outcome::CLIENT);
                }
                if item_tag(item, content, "minecraft:parrot_poisonous_food")? {
                    self.use_entity_food(item, hand, inventory, content)?;
                    let effect = content
                        .registry_entries("minecraft:mob_effect")?
                        .iter()
                        .position(|(name, _)| name == "minecraft:poison")
                        .map(|index| index as i32)
                        .ok_or("missing Source poison effect")?;
                    self.predict_use(UsePrediction::EntityEffect {
                        handle: target.entity.handle,
                        effect,
                        duration: 900,
                        amplifier: 0,
                        flags: 6,
                        tick,
                    })?;
                    // Entity.hurt's client implementation is a real no-op.
                    return Ok(Outcome::CLIENT);
                }
                Ok(
                    if target.entity.on_ground && target.tame()? && target.owned(self)? {
                        Outcome::CLIENT
                    } else {
                        Outcome::Pass
                    },
                )
            }
            "Panda" => {
                if target.number("MAIN_GENE_ID", 0)? == 2 && world.is_thundering()? {
                    return Ok(Outcome::Pass);
                }
                let flags = target.number("DATA_ID_FLAGS", 0)?;
                if flags & 16 != 0 {
                    self.predict_entity_metadata(
                        target,
                        "DATA_ID_FLAGS",
                        Metadata::Byte((flags & !16) as i8),
                    )?;
                    return Ok(Outcome::CLIENT);
                }
                if target.food(item, content)? {
                    if target.baby()? && !target.age_locked()? {
                        self.use_entity_food(item, hand, inventory, content)?;
                        let mut state = self.entity_use_state(target)?;
                        if state.forced_age_timer == 0 {
                            state.forced_age_timer = 40;
                        }
                        self.predict_entity_state(target, state)?;
                        return Ok(Outcome::CONSUME);
                    }
                    return Ok(Outcome::Pass);
                }
                if target.baby()?
                    && self.holding_entity_item(inventory, content, "minecraft:golden_dandelion")?
                {
                    self.animal_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    )
                } else {
                    Ok(Outcome::Pass)
                }
            }
            "Allay" => {
                self.allay_entity_interaction(target, item, hand, world, content, inventory, tick)
            }
            "Villager" | "WanderingTrader" => {
                // AbstractVillager.tradingPlayer is set only by server trading
                // methods. Opening a received screen does not set this field.
                let sleeping = matches!(target.metadata("DATA_POSE")?, Some(Metadata::Pose(2)));
                if name == "minecraft:villager_spawn_egg"
                    || !target.alive()?
                    || (owner == "Villager" && sleeping)
                    || (owner == "WanderingTrader" && target.baby()?)
                {
                    return self.ageable_entity_interaction(
                        target, item, hand, world, content, inventory, tick,
                    );
                }
                if owner == "Villager" && target.baby()? {
                    self.predict_entity_metadata(
                        target,
                        "DATA_UNHAPPY_COUNTER",
                        Metadata::Int(40),
                    )?;
                }
                Ok(Outcome::CLIENT)
            }
            "Creeper" => {
                if !item_tag(item, content, "minecraft:creeper_igniters")? {
                    return Ok(Outcome::Pass);
                }
                let pitch = target.store.entity_next_float(target.entity.handle)? * 0.4 + 0.8;
                self.play_use_level_sound(
                    world,
                    if name == "minecraft:fire_charge" {
                        "minecraft:item.firecharge.use"
                    } else {
                        "minecraft:item.flintandsteel.use"
                    },
                    crate::audio::SoundCategory::Hostile,
                    target.entity.position,
                    1.,
                    pitch,
                    true,
                )?;
                Ok(Outcome::CLIENT)
            }
            "Piglin" => {
                // Both admiration memories are un-synced and empty on the
                // client. ArmPose still depends on its actual synced/equipment
                // predicates, with dancing taking precedence over loved items.
                let admiring = !target.boolean("DATA_IS_DANCING", false)?
                    && item_tag(target.equipment(1), content, "minecraft:piglin_loved")?;
                Ok(
                    if !target.baby()? && name == "minecraft:gold_ingot" && !admiring {
                        Outcome::CLIENT
                    } else {
                        Outcome::Pass
                    },
                )
            }
            "ZombieVillager" => {
                if name != "minecraft:golden_apple" {
                    return Ok(Outcome::Pass);
                }
                let weakness = content
                    .registry_entries("minecraft:mob_effect")?
                    .iter()
                    .position(|(name, _)| name == "minecraft:weakness")
                    .map(|index| index as u32)
                    .ok_or("missing Source weakness effect")?;
                if target.entity.effects.contains_key(&weakness) {
                    self.consume_held(hand, inventory, 1)?;
                }
                Ok(Outcome::CONSUME)
            }
            _ => Err(format!("unknown Source mob interaction owner {owner}")),
        }
    }

    fn holding_entity_item(
        &self,
        inventory: &InventoryStore,
        content: &Content,
        name: &str,
    ) -> Result<bool> {
        for hand in 0..2 {
            if item_name(super::super::interaction::held(inventory, hand), content)? == name {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn entity_random_pitch(&self, target: &Target<'_>) -> Result<f32> {
        Ok(1.
            + (target.store.entity_next_float(target.entity.handle)?
                - target.store.entity_next_float(target.entity.handle)?)
                * 0.2)
    }

    fn entity_random_pitch_if_audible(&self, target: &Target<'_>) -> Result<Option<f32>> {
        if target.silent()? {
            Ok(None)
        } else {
            self.entity_random_pitch(target).map(Some)
        }
    }

    fn entity_null_sound(
        &mut self,
        target: &Target<'_>,
        world: &World,
        sound: &str,
        pitch: f32,
    ) -> Result<()> {
        if !target.silent()? {
            self.play_use_level_sound(
                world,
                sound,
                if target.is("Monster") {
                    crate::audio::SoundCategory::Hostile
                } else {
                    crate::audio::SoundCategory::Neutral
                },
                target.entity.position,
                1.,
                pitch,
                false,
            )?;
        }
        Ok(())
    }

    fn entity_happy_particle(&mut self, target: &Target<'_>, tick: u64) -> Result<()> {
        let bounds = target.store.bounds(target.entity.handle, tick, 1.)?;
        let width = bounds.max[0] - bounds.min[0];
        let height = bounds.max[1] - bounds.min[1];
        let mut samples = [UseParticleSample::default(); 16];
        samples[0].position = [
            target.entity.position[0]
                + (2. * target.store.entity_next_double(target.entity.handle)? - 1.) * width,
            target.entity.position[1]
                + target.store.entity_next_double(target.entity.handle)? * height
                + 0.5,
            target.entity.position[2]
                + (2. * target.store.entity_next_double(target.entity.handle)? - 1.) * width,
        ];
        self.predict_use(UsePrediction::Particles {
            kind: UseParticle::HappyVillager,
            samples,
            count: 1,
        })
    }
}

fn new_entity_item(content: &Content, name: &str) -> Result<Stack> {
    crate::inventory::creative_stack(content, content.item_id(name)?, 1)
}

impl Player {
    fn horse_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        let secondary = self.last_sent_input & 32 != 0;
        let baby = target.baby()?;
        let owner = target.owner("mobInteract")?;
        if owner == "SkeletonHorse" && !target.tame()? {
            return Ok(Outcome::Pass);
        }
        if owner == "Camel" {
            if secondary && !baby {
                return Ok(Outcome::CLIENT);
            }
            let result =
                self.living_item_interaction(target, item, hand, world, content, inventory)?;
            if result.consumes() {
                return Ok(result);
            }
            if target.food(item, content)? {
                return self.horse_entity_food(target, item, hand, world, content, inventory, tick);
            }
            if target.entity.passenger_ids.len() < 2 && !baby {
                self.horse_prepare_ride(target)?;
            }
            if baby && self.holding_entity_item(inventory, content, "minecraft:golden_dandelion")? {
                return self
                    .horse_base_interaction(target, item, hand, world, content, inventory, tick);
            }
            return Ok(Outcome::CONSUME);
        }
        let inventory_open = !baby && target.tame()? && secondary;
        if matches!(owner, "Horse" | "AbstractChestedHorse" | "ZombieHorse")
            && target.entity.passenger_ids.is_empty()
            && !inventory_open
            && (owner == "ZombieHorse"
                || !baby
                || !self.holding_entity_item(inventory, content, "minecraft:golden_dandelion")?)
        {
            if item.is_some() {
                if target.food(item, content)? {
                    return self
                        .horse_entity_food(target, item, hand, world, content, inventory, tick);
                }
                if !target.tame()? {
                    // makeMad explicitly does nothing on ClientLevel.
                    return Ok(Outcome::CLIENT);
                }
                if owner == "AbstractChestedHorse"
                    && !target.boolean("DATA_ID_CHEST", false)?
                    && item_name(item, content)? == "minecraft:chest"
                {
                    self.predict_entity_metadata(target, "DATA_ID_CHEST", Metadata::Bool(true))?;
                    let pitch = self.entity_random_pitch(target)?;
                    self.entity_null_sound(
                        target,
                        world,
                        if target.is("Llama") {
                            "minecraft:entity.llama.chest"
                        } else {
                            "minecraft:entity.donkey.chest"
                        },
                        pitch,
                    )?;
                    self.consume_held(hand, inventory, 1)?;
                    // createInventory copies its existing slots. Client mount
                    // storage has no server-only contents to fabricate here.
                    return Ok(Outcome::CLIENT);
                }
            }
        }
        self.horse_base_interaction(target, item, hand, world, content, inventory, tick)
    }

    fn horse_base_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        if !target.entity.passenger_ids.is_empty() || target.baby()? {
            return self
                .animal_entity_interaction(target, item, hand, world, content, inventory, tick);
        }
        if target.tame()? && self.last_sent_input & 32 != 0 {
            return Ok(Outcome::CLIENT);
        }
        if let Some(item) = item {
            let result =
                self.living_item_interaction(target, Some(item), hand, world, content, inventory)?;
            if result.consumes() {
                return Ok(result);
            }
            if target.equippable(item, 6, content)? && !target.wearing(6, content)? {
                let mut armor = item.clone();
                armor.count = 1;
                self.consume_held(hand, inventory, 1)?;
                self.predict_entity_equipment(target, 6, Some(armor))?;
                return Ok(Outcome::CLIENT);
            }
        }
        self.horse_prepare_ride(target)?;
        Ok(Outcome::CLIENT)
    }

    fn horse_prepare_ride(&mut self, target: &Target<'_>) -> Result<()> {
        let flags = target.number("DATA_ID_FLAGS", if target.is("Camel") { 2 } else { 0 })?;
        self.predict_entity_metadata(
            target,
            "DATA_ID_FLAGS",
            Metadata::Byte((flags & !(16 | 32)) as i8),
        )?;
        let mut state = self.entity_use_state(target)?;
        state.stand_counter = 0;
        self.predict_entity_state(target, state)
    }

    fn horse_entity_food(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        let name = item_name(item, content)?;
        let (heal, age_up) = if target.is("Camel") {
            (2., 10)
        } else if target.is("Llama") {
            match name {
                "minecraft:wheat" => (2., 10),
                "minecraft:hay_block" => (10., 90),
                _ => (0., 0),
            }
        } else {
            horse_food_amounts(name)
        };
        let mut used = heal > 0. && target.health()? < target.max_health()?;
        if used {
            self.heal_entity(target, heal)?;
        }
        let can_age_up = target.baby()? && age_up > 0 && !target.age_locked()?;
        if can_age_up {
            // AbstractHorse/Llama/Camel leave the age write behind a server
            // guard but still generate this actual client particle.
            self.entity_happy_particle(target, tick)?;
        }
        let mut state = self.entity_use_state(target)?;
        let client_love = target.is("Camel") && !target.is("CamelHusk")
            || target.is("Llama") && name == "minecraft:hay_block";
        if client_love && target.tame()? && !target.baby()? && state.in_love <= 0 {
            state.in_love = 600;
            self.predict_entity_state(target, state)?;
            used = true;
        }
        // Camel counts its unguarded canAgeUp predicate as item use. In
        // AbstractHorse/Llama only the server age write marks itemUsed.
        used |= target.is("Camel") && can_age_up;
        if used {
            if let Some(sound) = horse_eating_sound(target)? {
                if let Some(pitch) = self.entity_random_pitch_if_audible(target)? {
                    self.entity_null_sound(target, world, sound, pitch)?;
                }
            }
            self.consume_held(hand, inventory, 1)?;
        }
        // Source fedFood returns SUCCESS_SERVER on ClientLevel even when
        // handleEating did not mutate the stack.
        Ok(Outcome::CONSUME)
    }

    fn wolf_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        if target.tame()? {
            if target.food(item, content)? && target.health()? < target.max_health()? {
                self.feed_entity(target, item, hand, inventory, world, content, 2., 2.)?;
                return Ok(Outcome::CLIENT);
            }
            let owned = target.owned(self)?;
            if !item_tag(item, content, "minecraft:wolf_collar_dyes")? || !owned {
                if let Some(item) = item {
                    if target.equippable(item, 6, content)?
                        && !target.wearing(6, content)?
                        && owned
                        && !target.baby()?
                    {
                        let mut armor = item.clone();
                        armor.count = 1;
                        self.predict_entity_equipment(target, 6, Some(armor))?;
                        self.consume_held(hand, inventory, 1)?;
                        return Ok(Outcome::CLIENT);
                    }
                    if target.number("DATA_FLAGS_ID", 0)? & 1 != 0
                        && owned
                        && target.wearing(6, content)?
                    {
                        let armor = target
                            .equipment(6)
                            .ok_or("missing worn Source wolf armor")?;
                        let damage = component_integer(armor, "minecraft:damage", 0)?;
                        let max_damage = component_integer(armor, "minecraft:max_damage", 0)?;
                        if damage > 0
                            && max_damage > 0
                            && repair_holder_matches(armor, item, content)?
                        {
                            // Source shrink, unlike consume, spends materials
                            // in creative too and keeps the same stack identity.
                            let mut remaining = item.clone();
                            remaining.count = remaining.count.saturating_sub(1);
                            self.predict_held(hand, Some(remaining), inventory, false)?;
                            self.entity_null_sound(
                                target,
                                world,
                                "minecraft:entity.wolf.armor_repair",
                                1.,
                            )?;
                            let components = BTreeMap::from([(
                                "minecraft:damage".into(),
                                Value::from((damage - (max_damage as f32 * 0.125) as i32).max(0)),
                            )]);
                            let armor = crate::inventory::stack_with_components(
                                content,
                                armor,
                                &components,
                            )?;
                            self.predict_entity_equipment(target, 6, Some(armor))?;
                            return Ok(Outcome::CLIENT);
                        }
                    }
                }
                let result = self.animal_entity_interaction(
                    target, item, hand, world, content, inventory, tick,
                )?;
                if !result.consumes() && owned {
                    let mut state = self.entity_use_state(target)?;
                    state.ordered_to_sit = !state.ordered_to_sit;
                    self.predict_entity_state(target, state)?;
                    return Ok(Outcome::CLIENT_WITHOUT_ITEM);
                }
                return Ok(result);
            }
            if let Some(color) = dye(item)? {
                if color != target.number("DATA_COLLAR_COLOR", 14)? {
                    self.predict_entity_metadata(
                        target,
                        "DATA_COLLAR_COLOR",
                        Metadata::Int(color),
                    )?;
                    self.consume_held(hand, inventory, 1)?;
                    return Ok(Outcome::CLIENT);
                }
            }
        }
        // The untamed bone/anger branch is explicitly server-only.
        self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
    }

    fn allay_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        _tick: u64,
    ) -> Result<Outcome> {
        let equipment = target.equipment(0);
        if target.boolean("DATA_DANCING", false)?
            && item_tag(item, content, "minecraft:duplicates_allays")?
            && target.boolean("DATA_CAN_DUPLICATE", true)?
        {
            let mut state = self.entity_use_state(target)?;
            state.allay_duplication_cooldown = 6000;
            self.predict_entity_state(target, state)?;
            self.predict_entity_metadata(target, "DATA_CAN_DUPLICATE", Metadata::Bool(false))?;
            self.play_use_entity_sound(
                world,
                "minecraft:block.amethyst_block.chime",
                crate::audio::SoundCategory::Neutral,
                target.entity.handle,
                target.entity.position,
                2.,
                1.,
                true,
            )?;
            self.consume_held(hand, inventory, 1)?;
            return Ok(Outcome::CLIENT);
        }
        if equipment.is_none() {
            if let Some(item) = item {
                let mut given = item.clone();
                given.count = 1;
                self.predict_entity_equipment(target, 0, Some(given))?;
                self.consume_held(hand, inventory, 1)?;
                self.play_use_entity_sound(
                    world,
                    "minecraft:entity.allay.item_given",
                    crate::audio::SoundCategory::Neutral,
                    target.entity.handle,
                    target.entity.position,
                    2.,
                    1.,
                    true,
                )?;
                let mut state = self.entity_use_state(target)?;
                state.allay_liked_player = self.uuid.as_deref().map(str::to_owned);
                self.predict_entity_state(target, state)?;
                return Ok(Outcome::CLIENT);
            }
        } else if hand == 0 && item.is_none() {
            let taken = equipment.ok_or("missing Source allay held stack")?.clone();
            self.predict_entity_equipment(target, 0, None)?;
            self.play_use_entity_sound(
                world,
                "minecraft:entity.allay.item_taken",
                crate::audio::SoundCategory::Neutral,
                target.entity.handle,
                target.entity.position,
                2.,
                1.,
                true,
            )?;
            self.predict_use(UsePrediction::EntitySwing {
                handle: target.entity.handle,
                hand: 0,
            })?;
            let mut state = self.entity_use_state(target)?;
            state.allay_liked_player = None;
            self.predict_entity_state(target, state)?;
            // player.addItem, not handleExtraItemsCreatedOnUse: no dropping an
            // overflow stack, and creative does not discard this real item.
            self.predict_use(UsePrediction::ExtraItem {
                stack: taken,
                drop_remainder: false,
            })?;
            return Ok(Outcome::CLIENT);
        }
        Ok(Outcome::Pass)
    }
}

fn horse_food_amounts(name: &str) -> (f32, i32) {
    match name {
        "minecraft:wheat" => (2., 20),
        "minecraft:sugar" => (1., 30),
        "minecraft:hay_block" => (20., 180),
        "minecraft:apple" | "minecraft:carrot" => (3., 60),
        "minecraft:red_mushroom" => (3., 0),
        "minecraft:golden_carrot" => (4., 60),
        "minecraft:golden_apple" | "minecraft:enchanted_golden_apple" => (10., 240),
        _ => (0., 0),
    }
}

fn horse_eating_sound(target: &Target<'_>) -> Result<Option<&'static str>> {
    Ok(if target.is("CamelHusk") {
        Some("minecraft:entity.camel_husk.eat")
    } else if target.is("Camel") {
        Some("minecraft:entity.camel.eat")
    } else if target.is("ZombieHorse") {
        Some("minecraft:entity.zombie_horse.eat")
    } else if target.is("Horse") {
        Some(if target.baby()? {
            "minecraft:entity.horse.eat_baby"
        } else {
            "minecraft:entity.horse.eat"
        })
    } else if target.is("Llama") {
        Some("minecraft:entity.llama.eat")
    } else if target.is("Donkey") {
        Some("minecraft:entity.donkey.eat")
    } else if target.is("Mule") {
        Some("minecraft:entity.mule.eat")
    } else {
        None
    })
}

fn component_integer(stack: &Stack, name: &str, absent: i32) -> Result<i32> {
    stack.component(name).map_or(Ok(absent), |value| {
        value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("invalid Source integer component {name}"))
    })
}

fn repair_holder_matches(armor: &Stack, material: &Stack, content: &Content) -> Result<bool> {
    let Some(repairable) = armor.component("minecraft:repairable") else {
        return Ok(false);
    };
    let holders = repairable
        .get("items")
        .ok_or("Source repairable items missing")?;
    if let Some(name) = holders.as_str() {
        if let Some(tag) = name.strip_prefix('#') {
            return content.registry_tag_contains("minecraft:item", tag, material.id as i32);
        }
        return Ok(item_name(Some(material), content)? == name);
    }
    let holders = holders
        .as_array()
        .ok_or("invalid Source repairable holder set")?;
    let name = item_name(Some(material), content)?;
    Ok(holders.iter().any(|value| value.as_str() == Some(name)))
}

impl Player {
    fn bucket_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        _tick: u64,
    ) -> Result<Option<Outcome>> {
        if item_name(item, content)? != "minecraft:water_bucket" || !target.alive()? {
            return Ok(None);
        }
        let sound = if target.is("Axolotl") {
            "minecraft:item.bucket.fill_axolotl"
        } else if target.is("Tadpole") {
            "minecraft:item.bucket.fill_tadpole"
        } else {
            "minecraft:item.bucket.fill_fish"
        };
        self.entity_null_sound(target, world, sound, 1.)?;
        let bucket = target
            .store
            .bucket_item_stack(target.entity.handle, content)?;
        self.create_filled_result(
            item.ok_or("missing Source pickup water bucket")?,
            bucket,
            hand,
            inventory,
            false,
        )?;
        self.predict_use(UsePrediction::EntityDiscard {
            handle: target.entity.handle,
        })?;
        Ok(Some(Outcome::CLIENT))
    }

    fn mooshroom_entity_interaction(
        &mut self,
        target: &Target<'_>,
        item: Option<&Stack>,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        tick: u64,
    ) -> Result<Outcome> {
        let name = item_name(item, content)?;
        if name == "minecraft:bowl" && !target.baby()? {
            let mut state = self.entity_use_state(target)?;
            let suspicious = state.mooshroom_stew.take();
            let mut stew = new_entity_item(
                content,
                if suspicious.is_some() {
                    "minecraft:suspicious_stew"
                } else {
                    "minecraft:mushroom_stew"
                },
            )?;
            if let Some(effects) = suspicious.as_ref() {
                stew = crate::inventory::stack_with_components(
                    content,
                    &stew,
                    &BTreeMap::from([(
                        "minecraft:suspicious_stew_effects".into(),
                        effects.clone(),
                    )]),
                )?;
                self.predict_entity_state(target, state)?;
            }
            self.create_filled_result(
                item.ok_or("missing Source stew bowl")?,
                stew,
                hand,
                inventory,
                false,
            )?;
            self.entity_null_sound(
                target,
                world,
                if suspicious.is_some() {
                    "minecraft:entity.mooshroom.milk_suspiciously"
                } else {
                    "minecraft:entity.mooshroom.milk"
                },
                1.,
            )?;
            return Ok(Outcome::CLIENT);
        }
        if name == "minecraft:shears" && target.alive()? && !target.baby()? {
            // Conversion, drops, damage and shear sound require ServerLevel.
            return Ok(Outcome::CLIENT);
        }
        if target.number("DATA_TYPE", 0)? == 1 && !target.baby()? {
            if let Some(effects) = item
                .map(|item| content.suspicious_stew_effect(item.id))
                .transpose()?
                .flatten()
            {
                let mut state = self.entity_use_state(target)?;
                let occupied = state.mooshroom_stew.is_some();
                if !occupied {
                    self.consume_held(hand, inventory, 1)?;
                }
                let bounds = target.store.bounds(target.entity.handle, tick, 1.)?;
                let mut samples = [UseParticleSample::default(); 16];
                let count = if occupied { 2 } else { 4 };
                for sample in samples.iter_mut().take(count) {
                    sample.position = [
                        target.entity.position[0]
                            + target.store.entity_next_double(target.entity.handle)? / 2.,
                        target.entity.position[1] + (bounds.max[1] - bounds.min[1]) * 0.5,
                        target.entity.position[2]
                            + target.store.entity_next_double(target.entity.handle)? / 2.,
                    ];
                    sample.velocity[1] =
                        target.store.entity_next_double(target.entity.handle)? / 5.;
                }
                self.predict_use(UsePrediction::Particles {
                    kind: if occupied {
                        UseParticle::SmallSmoke
                    } else {
                        UseParticle::Effect {
                            color: -1,
                            alpha: 1.,
                        }
                    },
                    samples,
                    count: count as u8,
                })?;
                if !occupied {
                    state.mooshroom_stew = Some(effects);
                    self.predict_entity_state(target, state)?;
                    if !target.silent()? {
                        self.play_use_level_sound(
                            world,
                            "minecraft:entity.mooshroom.eat",
                            crate::audio::SoundCategory::Neutral,
                            target.entity.position,
                            2.,
                            1.,
                            false,
                        )?;
                    }
                }
                return Ok(Outcome::CLIENT);
            }
        }
        if name == "minecraft:bucket" && !target.baby()? {
            self.play_use_local_sound(world, "minecraft:entity.cow.milk", 1., 1.)?;
            self.create_filled_result(
                item.ok_or("missing Source milk bucket")?,
                new_entity_item(content, "minecraft:milk_bucket")?,
                hand,
                inventory,
                true,
            )?;
            return Ok(Outcome::CLIENT);
        }
        self.animal_entity_interaction(target, item, hand, world, content, inventory, tick)
    }

    fn entity_eating(
        &mut self,
        target: &Target<'_>,
        world: &World,
        _content: &Content,
    ) -> Result<()> {
        if target.is("Frog") {
            if !target.silent()? {
                self.play_use_level_sound(
                    world,
                    "minecraft:entity.frog.eat",
                    crate::audio::SoundCategory::Neutral,
                    target.entity.position,
                    2.,
                    1.,
                    false,
                )?;
            }
            return Ok(());
        }
        if target.is("Fox") {
            return self.entity_null_sound(target, world, "minecraft:entity.fox.eat", 1.);
        }
        if target.is("Goat") || target.is("Sniffer") {
            let pitch = 0.8 + world.next_float()? * 0.4;
            let sound = if target.is("Sniffer") {
                "minecraft:entity.sniffer.eat"
            } else if target.boolean("DATA_IS_SCREAMING_GOAT", false)? {
                "minecraft:entity.goat.screaming.eat"
            } else {
                "minecraft:entity.goat.eat"
            };
            return self.entity_null_sound(target, world, sound, pitch);
        }
        if target.is("Cat") {
            return self.entity_null_sound(
                target,
                world,
                target.store.eating_sound(target.entity.handle)?,
                1.,
            );
        }
        if target.is("Pig") || target.is("Armadillo") || target.is("AbstractNautilus") {
            let sound = if target.is("Pig") || target.is("AbstractNautilus") {
                target.store.eating_sound(target.entity.handle)?
            } else {
                "minecraft:entity.armadillo.eat"
            };
            let pitch = (target.store.entity_next_float(target.entity.handle)?
                - target.store.entity_next_float(target.entity.handle)?)
                * 0.2
                + if target.baby()? { 1.5 } else { 1. };
            return self.entity_null_sound(target, world, sound, pitch);
        }
        // Animal's default playEatingSound has no effect.
        Ok(())
    }
}
