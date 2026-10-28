use super::collision::{self, Bounds};
use super::*;
use crate::inventory::Stack;
const SPECTATOR_CANNOT_BREAK: &str = "spectator mode cannot break blocks";

pub(super) struct Breaking {
    pub hit: BlockHit,
    pub item: Option<Stack>,
    pub progress: f32,
    pub ticks: u32,
    pub finished: bool,
}
pub(super) struct UsingItem {
    pub hand: u8,
    pub item_id: u32,
    pub slot_identity: u64,
    pub duration: i32,
    pub remaining: i32,
    pub movement_factor: f32,
    pub can_sprint: bool,
}

impl Player {
    pub fn raycast(
        &self,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<Hit>> {
        if !self.pose_known {
            return Ok(None);
        }
        let origin = self.eye_position();
        let direction = self.look_direction();
        let block_reach = self.attribute("minecraft:block_interaction_range");
        let entity_reach = self.attribute("minecraft:entity_interaction_range");
        let maximum = block_reach.max(entity_reach);
        if !(0. ..=1024.).contains(&maximum) {
            return Err("interaction_reach_bound".into());
        }
        let block = self.raycast_blocks(world, content, maximum, false, false)?;
        let entity_limit = block
            .as_ref()
            .map_or(entity_reach, |b| entity_reach.min(b.distance));
        if let Some(entity) = entities.raycast_excluding(
            origin,
            direction,
            entity_limit,
            self.entity_id,
            self.vehicle,
            tick,
            0.,
        )? {
            return Ok(Some(Hit::Entity {
                handle: entity.handle,
                point: entity.point,
                distance: entity.distance,
            }));
        }
        Ok(block.filter(|b| b.distance <= block_reach).map(Hit::Block))
    }
    pub(super) fn raycast_blocks(
        &self,
        world: &World,
        content: &Content,
        maximum: f64,
        fluids: bool,
        source_only: bool,
    ) -> Result<Option<BlockHit>> {
        if !self.pose_known {
            return Ok(None);
        }
        if !(0. ..=1024.).contains(&maximum) {
            return Err("interaction_reach_bound".into());
        }
        // Level uses VOID_AIR both outside height and in EmptyLevelChunk.
        // Keep this Source-only projection separate from observed loadedness.
        let mut empty_state = None;
        let mut source_state = |position| -> Result<u32> {
            if let Some(state) = world.block(position) {
                return Ok(state);
            }
            if let Some(state) = empty_state {
                return Ok(state);
            }
            let state = content
                .resolve_block_state("minecraft:void_air", &std::collections::BTreeMap::new())?;
            empty_state = Some(state);
            Ok(state)
        };
        let origin = self.eye_position();
        let direction = self.look_direction();
        let mut cell = origin.map(|v| v.floor() as i32);
        let step = direction.map(|v| if v >= 0. { 1 } else { -1 });
        let delta = direction.map(|v| {
            if v.abs() < 1e-12 {
                f64::INFINITY
            } else {
                1. / v.abs()
            }
        });
        let mut next = std::array::from_fn::<_, 3, _>(|i| {
            if direction[i] > 0. {
                (f64::from(cell[i]) + 1. - origin[i]) * delta[i]
            } else {
                (origin[i] - f64::from(cell[i])) * delta[i]
            }
        });
        let mut distance = 0.;
        let mut block: Option<BlockHit> = None;
        for _ in 0..4096 {
            if distance > maximum {
                break;
            }
            let state = source_state(cell)?;
            let shapes = content
                .selection_boxes(state)
                .ok_or("unknown_selection_shape")?;
            let offset = content.block_offset(state, cell);
            let fluid = content
                .info(state)?
                .fluid
                .filter(|fluid| fluids && (!source_only || fluid.amount == 8 && !fluid.falling));
            let fluid_box = if let Some(fluid) = fluid {
                let above = source_state([cell[0], cell[1] + 1, cell[2]])?;
                let height = if content
                    .info(above)?
                    .fluid
                    .is_some_and(|above| above.kind == fluid.kind)
                {
                    1.
                } else {
                    f64::from(f32::from(fluid.amount) / 9.)
                };
                Some([0., 0., 0., 1., height, 1.])
            } else {
                None
            };
            for (shape, offset) in shapes
                .iter()
                .map(|shape| (shape, offset))
                .chain(fluid_box.iter().map(|shape| (shape, [0.; 3])))
            {
                let bounds = Bounds::new(*shape)
                    .moved(std::array::from_fn(|i| f64::from(cell[i]) + offset[i]));
                if let Some((hit_distance, face, inside)) =
                    collision::ray_box(origin, direction, bounds, maximum)
                {
                    if block.as_ref().is_none_or(|b| hit_distance < b.distance) {
                        block = Some(BlockHit {
                            position: cell,
                            face,
                            point: std::array::from_fn(|i| origin[i] + direction[i] * hit_distance),
                            state,
                            distance: hit_distance,
                            inside,
                        });
                    }
                }
            }
            if block
                .as_ref()
                .is_some_and(|b| b.distance <= next[0].min(next[1]).min(next[2]))
            {
                break;
            }
            let axis = if next[0] < next[1] && next[0] < next[2] {
                0
            } else if next[1] < next[2] {
                1
            } else {
                2
            };
            distance = next[axis];
            next[axis] += delta[axis];
            cell[axis] += step[axis];
        }
        Ok(block)
    }
    pub fn interaction_context(
        &self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<InteractionContext> {
        Ok(InteractionContext {
            pose_revision: self.pose_revision,
            inventory_revision: inventory.revision,
            target: self.raycast(world, content, entities, tick)?,
        })
    }
    pub fn execute(
        &mut self,
        intent: InteractionIntent,
        context: &InteractionContext,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        if matches!(intent, InteractionIntent::ReleaseAll) {
            return self.release_all(out);
        }
        if context.pose_revision != self.pose_revision {
            return Err("stale_player_pose".into());
        }
        if context.inventory_revision != inventory.revision {
            return Err("stale_inventory".into());
        }
        if inventory.screen_open {
            return Err("gameplay_screen_blocked".into());
        }
        if !self.pose_known {
            return Err("player_pose_not_received".into());
        }
        let target = self.raycast(world, content, entities, tick)?;
        if !same_hit(context.target.as_ref(), target.as_ref()) {
            return Err("stale_interaction_target".into());
        }
        self.perform(
            intent, target, world, content, inventory, entities, tick, out,
        )
    }
    fn perform(
        &mut self,
        intent: InteractionIntent,
        target: Option<Hit>,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        match intent {
            InteractionIntent::Attack => {
                self.attack(target, world, content, inventory, entities, tick, out)
            }
            InteractionIntent::Use { hand } => {
                if hand > 1 {
                    return Err("invalid_hand".into());
                }
                self.use_target(target, hand, world, content, inventory, entities, tick, out)
            }
            InteractionIntent::Pick { include_data } => {
                if include_data && self.game_mode != 1 {
                    return Err("creative_pick_data_required".into());
                }
                match target {
                    Some(Hit::Block(hit)) => {
                        let mut w = Writer::default();
                        w.i64(protocol::pack_pos(hit.position));
                        w.byte(u8::from(include_data));
                        out.push(PacketCommand::new(36, w));
                    }
                    Some(Hit::Entity { handle, .. }) => {
                        entities.resolve(handle)?;
                        let mut w = Writer::default();
                        w.var(handle.network_id);
                        w.byte(u8::from(include_data));
                        out.push(PacketCommand::new(37, w));
                    }
                    None => {}
                }
                Ok(())
            }
            InteractionIntent::Drop { whole_stack } => {
                if self.game_mode == 3 {
                    return Ok(());
                }
                self.action(
                    if whole_stack { 3 } else { 4 },
                    [0.; 3].map(|v| v as i32),
                    0,
                    false,
                    out,
                )
            }
            InteractionIntent::SwapHands => {
                if self.game_mode == 3 {
                    return Ok(());
                }
                self.stop_using(out)?;
                self.action(6, [0; 3], 0, false, out)
            }
            InteractionIntent::StartGliding => {
                if !self.can_glide(inventory, tick) {
                    return Err("cannot_start_gliding".into());
                }
                self.gliding = true;
                self.entity_command(8, 0, out);
                Ok(())
            }
            InteractionIntent::ToggleFlight => {
                if !self.abilities.may_fly {
                    return Err("flight_not_permitted".into());
                }
                if self.game_mode != 3 {
                    self.abilities.flying = !self.abilities.flying;
                    let mut w = Writer::default();
                    w.byte(if self.abilities.flying { 2 } else { 0 });
                    out.push(PacketCommand::new(40, w));
                }
                Ok(())
            }
            InteractionIntent::OpenMountInventory => {
                let handle = self.vehicle.ok_or("not_mounted")?;
                entities.resolve(handle)?;
                self.entity_command(7, 0, out);
                Ok(())
            }
            InteractionIntent::ReleaseAll => self.release_all(out),
        }
    }
    pub(super) fn interaction_tick(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        if self.break_delay > 0 {
            self.break_delay -= 1;
        }
        if self.use_delay > 0 {
            self.use_delay -= 1;
        }
        if self.miss_delay > 0 {
            self.miss_delay -= 1;
        }
        if !input.attack {
            self.abort_break(out)?;
            self.miss_delay = 0;
        }
        self.tick_using_item(world, content, inventory, entities, tick, out)?;
        if self.using.is_some() && !input.use_item {
            self.stop_using(out)?;
            self.apply_pending_use_release(world, content, inventory, entities, tick)?;
        }
        if self.using.is_some() {
            return Ok(());
        }
        if inventory.screen_open || !self.pose_known || self.health.is_some_and(|h| h <= 0.) {
            return Ok(());
        }
        if input.swap_hands && !self.last_input.swap_hands {
            self.perform(
                InteractionIntent::SwapHands,
                None,
                world,
                content,
                inventory,
                entities,
                tick,
                out,
            )?;
        }
        if input.drop && !self.last_input.drop {
            self.perform(
                InteractionIntent::Drop { whole_stack: false },
                None,
                world,
                content,
                inventory,
                entities,
                tick,
                out,
            )?;
        }
        if input.pick && !self.last_input.pick {
            let target = self.raycast(world, content, entities, tick)?;
            self.perform(
                InteractionIntent::Pick {
                    include_data: false,
                },
                target,
                world,
                content,
                inventory,
                entities,
                tick,
                out,
            )?;
        }
        if input.attack && self.miss_delay == 0 {
            let target = self.raycast(world, content, entities, tick)?;
            if !self.last_input.attack {
                self.attack(
                    target.clone(),
                    world,
                    content,
                    inventory,
                    entities,
                    tick,
                    out,
                )?;
            } else if let Some(Hit::Block(hit)) = target {
                self.break_block(hit, world, content, inventory, entities, tick, out)?;
            } else {
                self.abort_break(out)?;
            }
        }
        if input.use_item && self.use_delay == 0 {
            let target = self.raycast(world, content, entities, tick)?;
            let hand = u8::from(input.offhand);
            self.use_target(target, hand, world, content, inventory, entities, tick, out)?;
            self.use_delay = 4;
        }
        Ok(())
    }
    fn attack(
        &mut self,
        target: Option<Hit>,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        if self.using.is_some() {
            return Ok(());
        }
        if self.game_mode == 3 {
            if let Some(Hit::Entity { handle, .. }) = target {
                let mut w = Writer::default();
                w.var(handle.network_id.checked_add(1).ok_or("spectator entity id overflow")?);
                out.push(PacketCommand::new(63, w));
            }
            return Ok(());
        }
        if held(inventory, 0)
            .is_some_and(|s| s.components.contains_key("minecraft:piercing_weapon"))
        {
            self.action(7, [0; 3], 0, false, out)?;
            self.attack_ticks = 0;
            self.swing(0, out);
            return Ok(());
        }
        match target {
            Some(Hit::Entity { handle, .. }) => {
                self.abort_break(out)?;
                entities.resolve(handle)?;
                let mut w = Writer::default();
                w.var(handle.network_id);
                out.push(PacketCommand::new(1, w));
                self.attack_ticks = 0;
                self.swing(0, out);
            }
            Some(Hit::Block(hit)) => {
                self.break_block(hit, world, content, inventory, entities, tick, out)?
            }
            None => {
                self.abort_break(out)?;
                self.swing(0, out);
                self.attack_ticks = 0;
                if self.game_mode != 1 {
                    self.miss_delay = 10;
                }
            }
        }
        Ok(())
    }
    /// Gates that make an ordinary Attack on `hit` send nothing right now. Shared by
    /// `break_block` (which returns silently, like the vanilla client) and
    /// `attack_blocker` (which reports the reason to agents).
    fn break_gate(
        &self,
        hit: &BlockHit,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
    ) -> Result<Option<AttackBlocker>> {
        let refused = |reason| {
            Ok(Some(AttackBlocker {
                reason,
                transient: false,
            }))
        };
        if self.game_mode == 3 {
            return refused(SPECTATOR_CANNOT_BREAK);
        }
        if self.break_delay > 0 {
            return Ok(Some(AttackBlocker {
                reason: "destroy delay is active; it expires after a few client ticks",
                transient: true,
            }));
        }
        let item = held(inventory, 0);
        if self.game_mode == 2
            && !self.abilities.may_build
            && !super::adventure::matches(
                item,
                "minecraft:can_break",
                hit,
                world,
                entities,
                content,
            )?
        {
            return refused(
                "adventure mode: the held item has no minecraft:can_break entry matching this block",
            );
        }
        if self.game_mode == 1
            && item
                .and_then(|s| s.components.get("minecraft:tool"))
                .and_then(|v| v.get("can_destroy_blocks_in_creative"))
                .and_then(Value::as_bool)
                == Some(false)
        {
            return refused("creative mode: the held tool cannot destroy blocks");
        }
        Ok(None)
    }
    /// Why an ordinary Attack on `target` cannot start (or continue) mining right now,
    /// mirroring the gates of `attack`/`break_block`. `None` means mining starts, or is
    /// already in progress on that block, so an absent dig-start packet is not a refusal.
    pub fn attack_blocker(
        &self,
        target: Option<&Hit>,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
    ) -> Result<Option<AttackBlocker>> {
        let refused = |reason| {
            Ok(Some(AttackBlocker {
                reason,
                transient: false,
            }))
        };
        if self.using.is_some() {
            return refused("an item is being used; release use first");
        }
        if self.game_mode == 3 {
            return refused(SPECTATOR_CANNOT_BREAK);
        }
        let hit = match target {
            Some(Hit::Block(hit)) => hit,
            Some(Hit::Entity { .. }) => {
                return refused("an entity, not a block, is under the crosshair")
            }
            None => return refused("no block is under the crosshair within reach"),
        };
        if let Some(blocker) = self.break_gate(hit, world, content, inventory, entities)? {
            return Ok(Some(blocker));
        }
        if self.breaking.as_ref().is_some_and(|b| {
            b.finished
                && b.hit.position == hit.position
                && b.hit.state == hit.state
                && same_stack(b.item.as_ref(), held(inventory, 0))
        }) {
            return Ok(Some(AttackBlocker {
                reason: "the previous break of this block is finalizing; attack must be released for a tick",
                transient: true,
            }));
        }
        Ok(None)
    }
    fn break_block(
        &mut self,
        hit: BlockHit,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        if self
            .break_gate(&hit, world, content, inventory, entities)?
            .is_some()
        {
            return Ok(());
        }
        let item = held(inventory, 0);
        let info = content.info(hit.state)?;
        if self.breaking.as_ref().is_some_and(|b| {
            b.hit.position != hit.position
                || b.hit.state != hit.state
                || !same_stack(b.item.as_ref(), item)
        }) {
            self.abort_break(out)?;
        }
        if self.breaking.as_ref().is_some_and(|b| b.finished) {
            return Ok(());
        }
        let damage = self.destroy_progress(content, inventory, hit.state, tick)?;
        if self.breaking.is_none() {
            self.action(0, hit.position, hit.face, true, out)?;
            self.swing(0, out);
            if self.game_mode == 1 || damage >= 1. {
                self.break_delay = 5;
                self.emit_break_completion(world, content, &hit)?;
                self.breaking = Some(Breaking {
                    hit,
                    item: item.cloned(),
                    progress: 1.,
                    ticks: 0,
                    finished: true,
                });
                return Ok(());
            }
            self.breaking = Some(Breaking {
                hit,
                item: item.cloned(),
                progress: 0.,
                ticks: 0,
                finished: false,
            });
            return Ok(());
        }
        if info.hardness < 0. {
            self.swing(0, out);
            return Ok(());
        }
        let (hit, knock, finished) = {
            let b = self.breaking.as_mut().unwrap();
            b.progress += damage;
            b.ticks = b.ticks.saturating_add(1);
            (b.hit.clone(), b.ticks.is_multiple_of(4), b.progress >= 1.)
        };
        if knock {
            self.emit_break_hit(world, content, &hit)?;
        }
        self.swing(0, out);
        if finished {
            self.action(2, hit.position, hit.face, true, out)?;
            self.emit_break_completion(world, content, &hit)?;
            self.breaking.as_mut().unwrap().finished = true;
            self.break_delay = 5;
        }
        Ok(())
    }
    pub fn block_changed(&mut self, position: [i32; 3], state: u32) {
        if self
            .breaking
            .as_ref()
            .is_some_and(|b| b.hit.position == position && b.hit.state != state)
        {
            self.breaking = None;
        }
    }
    /// Local mining crack: same stage formula as breaking_observation
    /// (`(progress * 10) as i32 - 1`). None while idle/finished; caller clamps
    /// into 0..=9.
    pub fn breaking_crack(&self) -> Option<([i32; 3], i32)> {
        let breaking = self.breaking.as_ref()?;
        if breaking.finished {
            return None;
        }
        Some((breaking.hit.position, (breaking.progress * 10.) as i32 - 1))
    }
    pub fn breaking_observation(&self) -> Option<Value> {
        self.breaking.as_ref().map(|b|serde_json::json!({"position":b.hit.position,"state_id":b.hit.state,"progress":b.progress,"stage":(b.progress*10.) as i32-1,"finished_dispatched":b.finished,"sequence":self.sequence,"ack_sequence":self.ack_sequence,"origin":"locally_predicted"}))
    }
    pub fn using_observation(&self, _tick: u64) -> Option<Value> {
        self.using.as_ref().map(|u|serde_json::json!({"hand":u.hand,"item_id":u.item_id,"elapsed_ticks":u.duration.wrapping_sub(u.remaining),"duration":u.duration,"remaining":u.remaining,"origin":"locally_predicted"}))
    }
    pub(crate) fn is_scoping(&self, content: &Content) -> bool {
        self.using.as_ref().is_some_and(|using| {
            content.item_names.get(&using.item_id).map(String::as_str) == Some("minecraft:spyglass")
        })
    }
    pub fn attack_strength(&self, partial: f32) -> f32 {
        ((self.attack_ticks as f32 + partial)
            / (20. / self.attribute("minecraft:attack_speed") as f32))
            .clamp(0., 1.)
    }
    pub fn destroy_progress(
        &self,
        content: &Content,
        inventory: &InventoryStore,
        state: u32,
        tick: u64,
    ) -> Result<f32> {
        let info = content.info(state)?;
        if info.hardness < 0. {
            return Ok(0.);
        }
        if info.hardness == 0. {
            return Ok(1.);
        }
        let (speed, correct) = self.tool_properties(held(inventory, 0), content, state)?;
        let mut speed = speed;
        if speed > 1. {
            speed += self.attribute("minecraft:mining_efficiency") as f32;
        }
        let haste = self.amplifier("minecraft:haste", tick);
        let conduit = self.amplifier("minecraft:conduit_power", tick);
        if let Some(a) = haste.into_iter().chain(conduit).max() {
            speed *= 1. + (a + 1) as f32 * 0.2;
        }
        if let Some(a) = self.amplifier("minecraft:mining_fatigue", tick) {
            speed *= match a {
                0 => 0.3,
                1 => 0.09,
                2 => 0.0027,
                _ => 0.00081,
            };
        }
        speed *= self.attribute("minecraft:block_break_speed") as f32;
        if self.eye_in_water {
            speed *= self.attribute("minecraft:submerged_mining_speed") as f32;
        }
        if !self.grounded {
            speed /= 5.;
        }
        Ok(speed
            / info.hardness
            / if !info.requires_tool || correct {
                30.
            } else {
                100.
            })
    }
    fn tool_properties(
        &self,
        item: Option<&Stack>,
        content: &Content,
        state: u32,
    ) -> Result<(f32, bool)> {
        let Some(tool) = item.and_then(|s| s.components.get("minecraft:tool")) else {
            return Ok((1., false));
        };
        let rules = tool
            .get("rules")
            .and_then(Value::as_array)
            .ok_or("invalid_tool_rules")?;
        let mut speed = None;
        let mut correct = None;
        for rule in rules {
            if self.holder_matches(
                rule.get("blocks").ok_or("invalid_tool_blocks")?,
                content,
                state,
            )? {
                if speed.is_none() {
                    speed = rule.get("speed").and_then(Value::as_f64).map(|v| v as f32);
                }
                if correct.is_none() {
                    correct = rule.get("correct_for_drops").and_then(Value::as_bool);
                }
                if speed.is_some() && correct.is_some() {
                    break;
                }
            }
        }
        Ok((
            speed.unwrap_or_else(|| {
                tool.get("default_mining_speed")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.) as f32
            }),
            correct.unwrap_or(false),
        ))
    }
    pub(super) fn holder_matches(
        &self,
        set: &Value,
        content: &Content,
        state: u32,
    ) -> Result<bool> {
        content.holder_set_matches_block(set, state)
    }
    fn use_block_on(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<super::use_on::Outcome> {
        if hand > 1 {
            return Err("invalid_hand".into());
        }
        if !world.border_contains_block(hit.position) {
            return Ok(super::use_on::Outcome::Fail);
        }
        let sequence = self.next_sequence()?;
        let mut writer = Writer::default();
        writer.var(i32::from(hand));
        writer.i64(protocol::pack_pos(hit.position));
        writer.var(hit.face);
        for axis in 0..3 {
            writer.f32((hit.point[axis] - f64::from(hit.position[axis])) as f32);
        }
        writer.byte(u8::from(hit.inside));
        writer.byte(0);
        writer.var(sequence);
        out.push(PacketCommand::new(66, writer));
        let result = self.use_block_result(hit, hand, world, content, inventory, entities, tick)?;
        self.last_interaction_result = Some(result.java_result());
        Ok(result)
    }

    /// Agent useItemOn invokes the same vanilla block kernel as ordinary input,
    /// without Minecraft's separate air/offhand fallback, then swings that hand.
    pub fn minecraft_use_item_on(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<&'static str> {
        let result =
            self.use_block_on(hit, hand, world, content, inventory, entities, tick, out)?;
        self.swing(hand, out);
        self.visual.use_bump();
        Ok(result.java_result())
    }

    fn use_target(
        &mut self,
        target: Option<Hit>,
        first_hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        use super::use_on::{Outcome, UiTrigger, UsePrediction};
        self.last_interaction_result = None;
        if self.using.is_some()
            || self
                .breaking
                .as_ref()
                .is_some_and(|breaking| !breaking.finished)
        {
            return Ok(());
        }
        self.use_delay = 4;
        for hand in first_hand..=1 {
            let item = held(inventory, hand).filter(|item| !item.empty());
            if let Some(item) = item {
                if !content
                    .item_required_features(item.id)?
                    .iter()
                    .all(|feature| self.enabled_features.contains(feature))
                {
                    return Ok(());
                }
            }
            let result = match &target {
                Some(Hit::Entity { handle, point, .. }) => {
                    let entity = entities.resolve(*handle)?;
                    if !world.border_contains_block(
                        entity.position.map(|coordinate| coordinate.floor() as i32),
                    ) {
                        return Ok(());
                    }
                    let mut writer = Writer::default();
                    writer.var(handle.network_id);
                    writer.var(i32::from(hand));
                    write_lp_vec(
                        &mut writer,
                        std::array::from_fn(|axis| point[axis] - entity.position[axis]),
                    );
                    writer.byte(u8::from(self.last_sent_input & 32 != 0));
                    out.push(PacketCommand::new(26, writer));
                    if self.game_mode == 3 {
                        Outcome::Pass
                    } else {
                        self.entity_result(
                            *handle, *point, hand, world, content, inventory, entities, tick,
                        )?
                    }
                }
                Some(Hit::Block(hit)) => {
                    self.use_block_on(hit, hand, world, content, inventory, entities, tick, out)?
                }
                None => Outcome::Pass,
            };
            self.last_interaction_result = Some(result.java_result());
            if let Outcome::Success {
                swing,
                item_interaction,
            } = result
            {
                if swing {
                    self.swing(hand, out);
                    self.visual.use_bump();
                }
                let slot = if hand == 1 {
                    45
                } else {
                    36 + u16::from(inventory.selected_slot)
                };
                let changed_count =
                    self.use_predictions
                        .iter()
                        .any(|prediction| match prediction {
                            UsePrediction::PlayerSlot {
                                slot: changed,
                                stack,
                                ..
                            } if *changed == slot => {
                                stack.as_ref().map_or(0, |stack| stack.count)
                                    != item.map_or(0, |stack| stack.count)
                            }
                            _ => false,
                        });
                if matches!(target, Some(Hit::Block(_)))
                    && swing
                    && item_interaction
                    && item.is_some()
                    && (changed_count || self.abilities.instabuild)
                {
                    self.queue_use_ui(UiTrigger::ItemUsed { hand })?;
                }
                return Ok(());
            }
            if matches!(target, Some(Hit::Block(_))) && result == Outcome::Fail {
                return Ok(());
            }
            if let Some(item) = item.filter(|_| self.game_mode != 3) {
                let sequence = self.next_sequence()?;
                let mut writer = Writer::default();
                writer.var(i32::from(hand));
                writer.var(sequence);
                writer.f32(self.yaw);
                writer.f32(self.pitch);
                out.push(PacketCommand::new(67, writer));
                let result = if self.on_cooldown(item, content, tick)? {
                    Outcome::Pass
                } else {
                    self.air_result(
                        hand,
                        target.as_ref(),
                        world,
                        content,
                        inventory,
                        entities,
                        tick,
                    )?
                };
                self.last_interaction_result = Some(result.java_result());
                if let Outcome::Success {
                    swing,
                    item_interaction,
                } = result
                {
                    if swing {
                        self.swing(hand, out);
                        self.visual.use_bump();
                    }
                    if item_interaction {
                        self.queue_use_ui(UiTrigger::ItemUsed { hand })?;
                    }
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    pub(super) fn start_using(
        &mut self,
        item: Option<&Stack>,
        hand: u8,
        content: &Content,
        inventory: &InventoryStore,
        _tick: u64,
    ) -> Result<()> {
        if self.using.is_some() {
            return Ok(());
        }
        let Some(item) = item.filter(|item| !item.empty()) else {
            return Ok(());
        };
        let duration = self.use_duration(item, content)?;
        let effects = item.component("minecraft:use_effects");
        self.using = Some(UsingItem {
            hand,
            item_id: item.id,
            slot_identity: inventory.player_slot_identity(if hand == 1 {
                45
            } else {
                36 + u16::from(inventory.selected_slot)
            })?,
            duration,
            remaining: duration,
            movement_factor: effects
                .and_then(|value| value.get("speed_multiplier"))
                .and_then(Value::as_f64)
                .unwrap_or(0.2) as f32,
            can_sprint: effects
                .and_then(|value| value.get("can_sprint"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        Ok(())
    }
    pub(super) fn on_cooldown(&self, item: &Stack, content: &Content, tick: u64) -> Result<bool> {
        let name = content
            .item_names
            .get(&item.id)
            .ok_or("unknown_held_item")?;
        let group = item
            .components
            .get("minecraft:use_cooldown")
            .and_then(|v| v.get("cooldown_group"))
            .and_then(Value::as_str)
            .unwrap_or(name);
        Ok(self.cooldowns.get(group).is_some_and(|end| *end > tick))
    }
    fn next_sequence(&mut self) -> Result<i32> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("action_sequence_overflow")?;
        Ok(self.sequence)
    }
    fn action(
        &mut self,
        action: i32,
        position: [i32; 3],
        face: i32,
        predict: bool,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        let sequence = if predict { self.next_sequence()? } else { 0 };
        let mut w = Writer::default();
        w.var(if action == 0 { 0 } else { action + 1 });
        w.i64(protocol::pack_pos(position));
        w.var(face);
        w.var(sequence);
        out.push(PacketCommand::new(41, w));
        Ok(())
    }
    pub(super) fn swing(&self, hand: u8, out: &mut Vec<PacketCommand>) {
        self.visual.swing(hand);
        out.push(PacketCommand::new(46, Writer::default()));
    }
    fn abort_break(&mut self, out: &mut Vec<PacketCommand>) -> Result<()> {
        if let Some(b) = self.breaking.take() {
            if !b.finished {
                self.action(1, b.hit.position, b.hit.face, false, out)?;
            }
        }
        Ok(())
    }
    pub(super) fn stop_using(&mut self, out: &mut Vec<PacketCommand>) -> Result<()> {
        if let Some(using) = self.using.take() {
            self.action(5, [0; 3], 0, false, out)?;
            self.pending_use_release = Some(using);
        }
        Ok(())
    }
    pub fn release_all(&mut self, out: &mut Vec<PacketCommand>) -> Result<()> {
        self.abort_break(out)?;
        self.stop_using(out)?;
        if self.sent_sprinting {
            self.entity_command(4, 0, out);
            self.sent_sprinting = false;
        }
        if self.last_sent_input & 32 != 0 {
            self.entity_command(1, 0, out);
        }
        self.sync_input(HeldInput::default(), out);
        self.last_input = HeldInput::default();
        self.jump_trigger = 0;
        self.sprint_trigger = 0;
        self.sprinting = false;
        self.miss_delay = 0;
        Ok(())
    }
}
pub(super) fn held(inventory: &InventoryStore, hand: u8) -> Option<&Stack> {
    inventory
        .player
        .slots
        .get(if hand == 1 {
            45
        } else {
            36 + inventory.selected_slot as usize
        })
        .and_then(Option::as_ref)
}
fn same_stack(a: Option<&Stack>, b: Option<&Stack>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_item(b),
        _ => false,
    }
}
fn same_hit(a: Option<&Hit>, b: Option<&Hit>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Hit::Block(a)), Some(Hit::Block(b))) => {
            a.position == b.position && a.state == b.state && a.face == b.face
        }
        (Some(Hit::Entity { handle: a, .. }), Some(Hit::Entity { handle: b, .. })) => a == b,
        _ => false,
    }
}
pub(super) fn write_lp_vec(w: &mut Writer, vector: [f64; 3]) {
    let vector = vector.map(|v| v.clamp(-17179869183., 17179869183.));
    let maximum = vector.iter().map(|v| v.abs()).fold(0., f64::max);
    if maximum < 3.051944088384301e-5 {
        w.byte(0);
        return;
    }
    let scale = maximum.ceil() as u64;
    let continuation = scale > 3;
    let mut packed = (scale & 3) | if continuation { 4 } else { 0 };
    for i in 0..3 {
        let data = ((vector[i] / scale as f64 * 0.5 + 0.5) * 32766. + 0.5).floor() as u64;
        packed |= data << (3 + 15 * i);
    }
    w.byte(packed as u8);
    w.byte((packed >> 8) as u8);
    w.0.extend(((packed >> 16) as u32).to_be_bytes());
    if continuation {
        w.var((scale >> 2) as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_mining_emits_four_tick_hits_and_one_completion_burst() {
        let mut content = Content::test_cube();
        content.insert_test_block_sound(
            1,
            1.,
            1.,
            BTreeMap::from([
                ("hit".into(), "test:hit".into()),
                ("break".into(), "test:break".into()),
            ]),
        );
        let world = World::new(0, 16);
        let inventory = InventoryStore::default();
        let entities = EntityStore::new();
        let hit = BlockHit {
            position: [0, 3, 0],
            state: 1,
            face: 1,
            point: [0.5, 4., 0.5],
            distance: 2.,
            inside: false,
        };
        for creative in [false, true] {
            let mut player = Player::new();
            player.grounded = true;
            player.game_mode = if creative { 1 } else { 0 };
            let mut commands = Vec::new();
            let mut hits = 0;
            let mut completions = 0;
            let mut breaks = 0;
            for tick in 0..200 {
                player
                    .break_block(
                        hit.clone(),
                        &world,
                        &content,
                        &inventory,
                        &entities,
                        tick,
                        &mut commands,
                    )
                    .unwrap();
                while let Some(effect) = player.take_use_prediction() {
                    match effect {
                        UsePrediction::Sound(sound) if sound.event == "test:hit" => hits += 1,
                        UsePrediction::Sound(sound) if sound.event == "test:break" => breaks += 1,
                        UsePrediction::LevelEvent {
                            event: 2001,
                            position,
                            data: 1,
                        } => {
                            assert_eq!(position, hit.position);
                            completions += 1;
                        }
                        other => panic!("unexpected mining effect: {other:?}"),
                    }
                }
            }
            let breaking = player.breaking.as_ref().expect("completed mining state");
            assert!(breaking.finished);
            assert_eq!(hits, breaking.ticks / 4);
            assert_eq!(breaks, 1);
            assert_eq!(completions, 1);
        }
    }

    #[test]
    fn compressed_interact_location_roundtrip_covers_zero_negative_and_continuation() {
        for v in [[0.; 3], [-0.75, 1.2, 0.1], [9., -3., 4.]] {
            let mut w = Writer::default();
            write_lp_vec(&mut w, v);
            let result = read_velocity(&mut Reader::new(&w.0)).unwrap();
            for i in 0..3 {
                assert!((v[i] - result[i]).abs() < 0.001);
            }
        }
    }
    #[test]
    fn release_aborts_only_active_prediction_and_emits_stop_using_once() {
        let mut p = Player::new();
        p.breaking = Some(Breaking {
            hit: BlockHit {
                position: [-1, 2, 3],
                state: 4,
                face: 5,
                point: [0.; 3],
                distance: 1.,
                inside: false,
            },
            item: None,
            progress: 0.5,
            ticks: 3,
            finished: false,
        });
        let mut out = Vec::new();
        p.release_all(&mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 41);
        let mut r = Reader::new(&out[0].payload.0);
        assert_eq!(r.var().unwrap(), 1);
        assert_eq!(protocol::unpack_pos(r.i64().unwrap()), [-1, 2, 3]);
        p.release_all(&mut out).unwrap();
        assert_eq!(out.len(), 1);
    }
}
