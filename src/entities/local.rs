//! Predicted local avatar is presentation-only: never enters the received entity
//! index, spatial queries, observations or network-event history.
use super::*;
use crate::{
    inventory::InventoryStore,
    player::{Player, Pose},
    protocol::world::World,
};

impl Store {
    pub(crate) fn sync_local_avatar(
        &mut self,
        player: &Player,
        inventory: &InventoryStore,
        world: &World,
        content: &Content,
        clocks: (u64, i64),
    ) -> Result<()> {
        let Some(models) = self.models.clone() else {
            return Ok(());
        };
        let Some(uuid) = player.uuid.as_deref() else {
            return Ok(());
        };
        let (tick, game_time) = clocks;
        if self
            .local_avatar
            .as_ref()
            .is_none_or(|e| e.handle.network_id != player.entity_id || e.uuid != uuid)
        {
            let transform = Transform {
                position: player.position,
                yaw: player.yaw,
                pitch: player.pitch,
                head_yaw: player.yaw,
            };
            let kind = content
                .entity_names
                .iter()
                .find(|(_, name)| name.as_str() == "minecraft:player")
                .map(|(kind, _)| *kind)
                .ok_or("local player entity type missing")?;
            let mut entity = Entity {
                handle: Handle {
                    network_id: player.entity_id,
                    generation: self.generation,
                },
                uuid: uuid.into(),
                kind,
                type_name: "minecraft:player".into(),
                position: player.position,
                velocity: player.velocity,
                transform,
                previous: transform,
                on_ground: player.grounded,
                spawn_data: 0,
                observed_tick: tick,
                revision: 0,
                appearance_revision: 0,
                physical: physical::AnimationState::default(),
                binding: None,
                source: None,
                minecart_steps: vec![],
                acceleration_power: None,
                metadata: BTreeMap::new(),
                equipment: std::array::from_fn(|_| None),
                predicted_metadata: BTreeMap::new(),
                predicted_equipment: std::array::from_fn(|_| None),
                predicted_leash: None,
                effects: BTreeMap::new(),
                attributes: BTreeMap::new(),
                passenger_ids: vec![],
                passengers: vec![],
                vehicle: player.vehicle,
                leash: None,
                leash_holder_id: None,
                animation: None,
                damage_tick: None,
                death_tick: None,
                hurt_direction: 0.,
                submitted_revision: None,
            };
            entity.physical.lerp_duration = 1;
            entity.source = Some(source_state::SourceState::new(&entity, &models)?);
            self.local_avatar = Some(entity);
            self.local_avatar_tick = None;
        }
        let e = self.local_avatar.as_mut().unwrap();
        if self.local_avatar_tick == Some(tick) {
            // Mouse look is frame-driven, unlike movement and source animation.
            e.transform.yaw = player.yaw;
            e.transform.pitch = player.pitch;
            e.transform.head_yaw = player.yaw;
            return Ok(());
        }
        e.previous = e.transform;
        e.transform = Transform {
            position: player.position,
            yaw: player.yaw,
            pitch: player.pitch,
            head_yaw: player.yaw,
        };
        e.position = player.position;
        e.velocity = player.velocity;
        e.on_ground = player.grounded;
        e.vehicle = player.vehicle;
        e.observed_tick = tick;
        e.damage_tick = player.hurt_tick;
        e.animation = player.visual_swing(tick);
        e.death_tick = player.health.filter(|h| *h <= 0.).map(|_| tick);
        let pose = match player.pose {
            Pose::Standing => 0,
            Pose::FallFlying => 1,
            Pose::Sleeping => 2,
            Pose::Swimming => 3,
            Pose::SpinAttack => 4,
            Pose::Crouching => 5,
            Pose::Dying => 7,
        };
        if let Some(index) = models.accessor("minecraft:player", "DATA_POSE") {
            e.metadata.insert(index, Metadata::Pose(pose));
        }
        if let Some(index) = models.accessor("minecraft:player", "DATA_PLAYER_MODE_CUSTOMISATION") {
            e.metadata.insert(index, Metadata::Byte(127));
        }
        for (slot, inventory_slot) in [
            (0, 36 + usize::from(inventory.selected_slot)),
            (1, 45),
            (2, 8),
            (3, 7),
            (4, 6),
            (5, 5),
        ] {
            let stack = inventory
                .player
                .slots
                .get(inventory_slot)
                .and_then(Option::as_ref)
                .filter(|stack| !stack.empty());
            if e.equipment[slot].as_ref() != stack {
                e.equipment[slot] = stack.cloned();
            }
        }
        e.revision = e.revision.wrapping_add(1);
        // Tick source animation at the newly predicted position, then render
        // the same previous/current tick pair as the first-person camera.
        e.physical.lerp_progress = 1;
        source_state::tick(
            e,
            &models,
            world,
            content,
            game_time,
            tick,
            false,
            false,
            player.bounds(),
        )?;
        e.physical.lerp_progress = 0;
        self.local_avatar_tick = Some(tick);
        Ok(())
    }

    pub(crate) fn set_local_visible(&mut self, visible: bool) {
        self.render_local_avatar = visible;
    }

    fn local_skin(&mut self, content: &Content) -> Result<crate::protocol::content::TextureRegion> {
        let models = self
            .models
            .as_ref()
            .ok_or("local avatar models unavailable")?;
        let entity = self
            .local_avatar
            .as_ref()
            .ok_or("local avatar not synchronized")?;
        if entity.binding.is_none() {
            let binding = presentation::material(self, models, entity, content);
            self.local_avatar.as_mut().unwrap().binding = Some(binding);
        }
        self.local_avatar
            .as_ref()
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .as_ref()
            .map(|binding| binding.texture)
            .map_err(Clone::clone)
    }

    pub(crate) fn append_local_arm(
        &mut self,
        content: &Content,
        right: bool,
        root: Mat4,
        light: [f32; 2],
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let texture = self.local_skin(content)?;
        let binding = self
            .local_avatar
            .as_ref()
            .unwrap()
            .binding
            .as_ref()
            .unwrap()
            .as_ref()
            .map_err(Clone::clone)?;
        let rig = self
            .models
            .as_ref()
            .unwrap()
            .rigs
            .get(&binding.rig)
            .ok_or("local player rig is not activated")?;
        let name = if right { "right_arm" } else { "left_arm" };
        let bone = rig
            .bones
            .iter()
            .find(|bone| bone.name == name)
            .ok_or("local player rig has no arm bone")?;
        // Keep source-baked slim/wide geometry, skin UVs and normals. The caller
        // supplies a shoulder transform with the fist visible above the sleeve.
        let matrix = root * resources::transform(glam::Vec3::ZERO, bone.rotation, bone.scale);
        let normal_matrix = matrix.inverse().transpose();
        for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
            let first = out.vertices.len();
            presentation::emit(
                quad.map(|v| {
                    matrix
                        .transform_point3(glam::Vec3::new(v[0], v[1], v[2]))
                        .to_array()
                }),
                quad.map(|v| {
                    [
                        texture.uv[0] + v[3] * (texture.uv[2] - texture.uv[0]),
                        texture.uv[1] + v[4] * (texture.uv[3] - texture.uv[1]),
                    ]
                }),
                [1.; 3],
                texture.layer,
                crate::api::EntityMaterial::EntityCutout,
                1.,
                light,
                out,
            )?;
            let normal = normal_matrix
                .transform_vector3(glam::Vec3::from_array(*normal))
                .normalize_or_zero()
                .to_array();
            for vertex in &mut out.vertices[first..] {
                vertex.normal = normal;
            }
        }
        Ok(())
    }
}
