//! Borrowed 26.1.2 nameplate selection; shaping and component ownership belong to the cache.
//!
//! Source: local mapped client javap, EntityRenderer/LivingEntityRenderer/AvatarRenderer
//! (labels.javap), MobRenderer/ArmorStandRenderer (renderers.javap), Entity/Player
//! getName/getDisplayName/shouldShowName/isDiscrete/isInvisibleTo, and
//! EntityAttachments.transformPoint (attributes-attachments.javap). ItemFrameRenderer,
//! ItemStack.getCustomName/getHoverName and EnderDragonRenderer were checked against
//! /home/kaiserroman/research/client-26.1.2.jar. No original/derived Java bytes are included.
use super::{Entity, Handle, Metadata, Models, Store, presentation};
use crate::{
    protocol::Result,
    ui::{ScoreTeam, ScoreboardState, TeamParameters, TeamVisibility},
};
use serde_json::Value;

pub(crate) struct NameplateView<'a> {
    pub crosshair: Option<Handle>,
    pub camera_entity: Option<i32>,
    pub viewer_name: Option<&'a str>,
    pub viewer_spectator: bool,
    pub render_names: bool,
    pub scoreboard: &'a ScoreboardState,
    pub background_opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NameSource<'a> {
    Component(&'a Value),
    Player(&'a str),
    /// Entity type identifier; conversion to a translation key is cache-dirty work.
    Translation(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Nameplate<'a> {
    pub name: NameSource<'a>,
    pub team: Option<&'a TeamParameters>,
    /// Scaled, yaw-rotated local NAME_TAG[0], including a renderer's local offset.
    /// The geometry pass adds entity position and the source's unscaled +0.5 Y.
    pub attachment: [f64; 3],
    pub discrete: bool,
}

fn custom_name<'a>(models: &Models, entity: &'a Entity) -> Option<&'a Value> {
    match presentation::value(models, entity, "DATA_CUSTOM_NAME") {
        Some(Metadata::OptionalComponent(Some(name))) => Some(name),
        _ => None,
    }
}

fn shared_flags(models: &Models, entity: &Entity) -> u8 {
    match presentation::value(models, entity, "DATA_SHARED_FLAGS_ID") {
        Some(Metadata::Byte(flags)) => *flags as u8,
        _ => 0,
    }
}

fn allied(team: Option<&ScoreTeam>, viewer_team: Option<&ScoreTeam>) -> bool {
    match (team, viewer_team) {
        // Source Team.isAlliedTo and Entity.isInvisibleTo compare team identity.
        (Some(team), Some(viewer_team)) => std::ptr::eq(team, viewer_team),
        _ => false,
    }
}

fn living_visible(
    store: &Store,
    entity: &Entity,
    view: &NameplateView<'_>,
    distance_squared: f64,
    flags: u8,
    team: Option<&ScoreTeam>,
    viewer_team: Option<&ScoreTeam>,
) -> bool {
    if flags & 2 != 0 && distance_squared >= 1024. {
        return false;
    }
    let same_team = allied(team, viewer_team);
    let friendly_invisibles = team.is_some_and(|t| t.parameters.options & 2 != 0);
    // Entity.isInvisibleTo: spectators, or teammates allowed to see friendly
    // invisibles, see an invisible entity. Other viewers use shared flag bit 5.
    let visible = view.viewer_spectator || same_team && friendly_invisibles || flags & 32 == 0;
    // LivingEntityRenderer's team branches return before the ordinary camera,
    // renderNames and isVehicle gates. They do not apply to the local player.
    if store.local_entity_id != Some(entity.handle.network_id) {
        if let Some(team) = team {
            return match team.parameters.visibility {
                TeamVisibility::Always => visible,
                TeamVisibility::Never => false,
                TeamVisibility::HideForOtherTeams => {
                    if viewer_team.is_none() {
                        visible
                    } else {
                        same_team && (friendly_invisibles || visible)
                    }
                }
                TeamVisibility::HideForOwnTeam => {
                    if viewer_team.is_none() {
                        visible
                    } else {
                        !same_team && visible
                    }
                }
            };
        }
    }
    view.render_names
        && view.camera_entity != Some(entity.handle.network_id)
        && visible
        && entity.passenger_ids.is_empty()
}

fn frame_name<'a>(models: &Models, entity: &'a Entity) -> Option<&'a Value> {
    let Metadata::Item(stack) = presentation::value(models, entity, "DATA_ITEM")? else {
        return None;
    };
    if stack.empty() {
        return None;
    }
    if let Some(name) = stack.component("minecraft:custom_name") {
        return Some(name);
    }
    // ItemStack.getCustomName also returns a written book's nonblank raw title.
    // A retained JSON string is already a literal component, requiring no copy.
    let title = &stack.component("minecraft:written_book_content")?["title"]["raw"];
    let text = title.as_str()?;
    // StringUtil.isBlank uses Character.isWhitespace || Character.isSpaceChar.
    let blank = text.chars().all(|c| {
        matches!(c, '\u{0009}'..='\u{000d}' | '\u{001c}'..='\u{0020}'
            | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
    });
    (!blank).then_some(title)
}

pub(crate) fn nameplate<'a>(
    store: &'a Store,
    models: &Models,
    entity: &'a Entity,
    view: &NameplateView<'a>,
    distance_squared: f64,
    tick: u64,
    partial: f32,
) -> Result<Option<Nameplate<'a>>> {
    // EntityRenderer.extractRenderState applies this strict range before dynamic
    // shouldShowName dispatch, including ArmorStandRenderer's override.
    if !(distance_squared < 4096.) {
        return Ok(None);
    }
    let schema = models
        .schemas
        .get(&entity.type_name)
        .ok_or("entity nameplate source not activated")?;
    let inherits = |class: &str| schema.hierarchy.iter().any(|entry| entry == class);
    let flags = shared_flags(models, entity);
    let custom = custom_name(models, entity);
    let always = presentation::boolean(models, entity, "DATA_CUSTOM_NAME_VISIBLE");
    let targeted = view.crosshair == Some(entity.handle);
    let player = entity.type_name == "minecraft:player";
    let frame = matches!(
        entity.type_name.as_str(),
        "minecraft:item_frame" | "minecraft:glow_item_frame"
    );
    let profile_name = if player {
        Some(
            store
                .profiles
                .get(&entity.uuid)
                .ok_or("nameplate player profile missing")?
                .name
                .as_str(),
        )
    } else {
        None
    };
    // Entity.getScoreboardName uses UUID; Player overrides it with profile.name.
    let team = view.scoreboard.team(profile_name.unwrap_or(&entity.uuid));
    let viewer_team = view.viewer_name.and_then(|name| view.scoreboard.team(name));
    let requested = player || always || custom.is_some() && targeted;
    let frame_name = if frame {
        frame_name(models, entity)
    } else {
        None
    };
    let show = if entity.type_name == "minecraft:armor_stand" {
        // This override intentionally bypasses ALL Living renderer gates and the
        // custom-name crosshair fallback. The flag can show the translated type.
        always
    } else if frame {
        view.render_names && targeted && frame_name.is_some()
    } else if entity.type_name == "minecraft:ender_dragon" {
        // EnderDragon is a Mob, but its renderer extends EntityRenderer directly.
        requested
    } else if inherits("net.minecraft.world.entity.LivingEntity") {
        living_visible(
            store,
            entity,
            view,
            distance_squared,
            flags,
            team,
            viewer_team,
        ) && (!(inherits("net.minecraft.world.entity.Mob")
            || inherits("net.minecraft.world.entity.Avatar"))
            || requested)
    } else {
        requested
    };
    if !show {
        return Ok(None);
    }
    let (dimensions, scale) = models.dimensions(entity)?;
    let Some(point) = dimensions
        .attachments
        .get("NAME_TAG")
        .and_then(|points| points.first())
    else {
        // EntityAttachments.getNullable may return null; Storage.add skips it.
        return Ok(None);
    };
    let [x, y, z] = point.map(|v| v * f64::from(scale));
    let mut attachment = if x == 0. && z == 0. {
        [x, y, z]
    } else {
        // transformPoint -> Vec3.yRot(-yaw * DEG_TO_RAD), using Mth's source
        // quantized trigonometry and f32 angle before promotion to double.
        let angle = f64::from(-entity.presented_transform(tick, partial).yaw * 0.017453292);
        let sin = f64::from(crate::player::math::sin(angle));
        let cos = f64::from(crate::player::math::cos(angle));
        [x * cos + z * sin, y, z * cos - x * sin]
    };
    if frame {
        let direction = super::physical::direction(
            // HangingEntity.defineSynchedData initializes Direction.SOUTH.
            presentation::number(models, entity, "DATA_DIRECTION").unwrap_or(3),
        )?;
        attachment[0] += direction[0] * f64::from(0.3_f32);
        attachment[1] -= 0.25;
        attachment[2] += direction[2] * f64::from(0.3_f32);
    }
    let name = if let Some(name) = frame_name {
        NameSource::Component(name)
    } else if let Some(name) = profile_name {
        // Player.getName is always the literal game-profile name, never customName
        // or PlayerInfo's tab-list display_name.
        NameSource::Player(name)
    } else if let Some(name) = custom {
        NameSource::Component(name)
    } else {
        NameSource::Translation(&entity.type_name)
    };
    Ok(Some(Nameplate {
        name,
        // EntityRenderer.getNameTag calls getDisplayName, which decorates using
        // PlayerTeam.formatNameForTeam. ItemFrameRenderer returns item hoverName.
        team: if frame {
            None
        } else {
            team.map(|team| &team.parameters)
        },
        attachment,
        discrete: flags & 2 != 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::{PlayerProfile, Transform, physical, resources};
    use super::*;
    use crate::{
        inventory::Stack,
        ui::{ScoreTeam, TeamCollision},
    };
    use serde_json::json;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    };

    fn fixture(kind: &str, hierarchy: &[&str]) -> (Store, Models, Entity) {
        // Authored typed source-schema fixture, not copied cache/JAR bytes.
        let dimensions = resources::Dimensions {
            width: 1.,
            height: 2.,
            eye_height: 1.5,
            fixed: false,
            attachments: BTreeMap::from([("NAME_TAG".into(), vec![[0., 2., 0.]])]),
        };
        let mut constants = BTreeMap::new();
        for (name, point) in [
            ("BABY_DIMENSIONS", [0., 1., 0.]),
            ("MARKER_DIMENSIONS", [0.; 3]),
        ] {
            let mut value = dimensions.clone();
            value.attachments.insert("NAME_TAG".into(), vec![point]);
            constants.insert(name.into(), resources::DimensionConstant::Dimensions(value));
        }
        let schema = resources::EntitySchema {
            protocol_id: 1,
            width: 1.,
            fire_immune: false,
            class: hierarchy
                .first()
                .copied()
                .unwrap_or("net.minecraft.world.entity.Entity")
                .into(),
            eye_height: 1.5,
            dimension_constants: constants,
            enum_ids: BTreeMap::new(),
            enum_numbers: BTreeMap::new(),
            hierarchy: hierarchy.iter().map(|s| (*s).into()).collect(),
            method_owners: BTreeMap::new(),
            dimensions,
            default_attributes: BTreeMap::from([(
                "minecraft:scale".into(),
                resources::DefaultAttribute { id: 7, value: 1. },
            )]),
            height: 2.,
            accessors: BTreeMap::from([
                ("DATA_SHARED_FLAGS_ID".into(), [0, 0]),
                ("DATA_CUSTOM_NAME".into(), [2, 6]),
                ("DATA_CUSTOM_NAME_VISIBLE".into(), [3, 8]),
                ("DATA_POSE".into(), [6, 20]),
                ("DATA_CLIENT_FLAGS".into(), [15, 0]),
                ("DATA_DIRECTION".into(), [8, 12]),
                ("DATA_ITEM".into(), [9, 7]),
            ]),
        };
        let models = Models {
            rigs: BTreeMap::new(),
            schemas: BTreeMap::from([(kind.into(), schema)]),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            poses: BTreeMap::from([(0, "STANDING".into())]),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            presentation: BTreeMap::new(),
            animations: BTreeMap::new(),
            motion: BTreeMap::new(),
        };
        let transform = Transform {
            position: [11., 20., -4.],
            yaw: 0.,
            pitch: 0.,
            head_yaw: 0.,
        };
        let entity = Entity {
            handle: Handle {
                network_id: 41,
                generation: 9,
            },
            uuid: "00000000-0000-0000-0000-000000000041".into(),
            kind: 1,
            type_name: kind.into(),
            position: transform.position,
            velocity: [0.; 3],
            transform,
            previous: transform,
            on_ground: false,
            spawn_data: 0,
            observed_tick: 0,
            revision: 1,
            appearance_revision: 1,
            physical: physical::AnimationState::default(),
            binding: None,
            source: None,
            minecart_steps: Vec::new(),
            acceleration_power: None,
            metadata: BTreeMap::new(),
            equipment: std::array::from_fn(|_| None),
            predicted_metadata: BTreeMap::new(),
            predicted_equipment: std::array::from_fn(|_| None),
            predicted_leash: None,
            effects: BTreeMap::new(),
            attributes: BTreeMap::new(),
            passenger_ids: Vec::new(),
            passengers: Vec::new(),
            vehicle: None,
            leash: None,
            leash_holder_id: None,
            animation: None,
            damage_tick: None,
            death_tick: None,
            hurt_direction: 0.,
            submitted_revision: None,
        };
        (Store::default(), models, entity)
    }

    fn view(scoreboard: &ScoreboardState) -> NameplateView<'_> {
        NameplateView {
            crosshair: None,
            camera_entity: Some(1),
            viewer_name: Some("Viewer"),
            viewer_spectator: false,
            render_names: true,
            scoreboard,
            background_opacity: 0.25,
        }
    }

    fn selected<'a>(
        store: &'a Store,
        models: &Models,
        entity: &'a Entity,
        view: &NameplateView<'a>,
        distance: f64,
    ) -> Option<Nameplate<'a>> {
        nameplate(store, models, entity, view, distance, 0, 1.).unwrap()
    }

    fn visible_name(entity: &mut Entity) {
        entity.metadata.insert(
            2,
            Metadata::OptionalComponent(Some(json!({"text":"Received name"}))),
        );
        entity.metadata.insert(3, Metadata::Bool(true));
    }

    fn team(
        scoreboard: &mut ScoreboardState,
        holder: &str,
        key: &str,
        visibility: TeamVisibility,
        options: u8,
    ) {
        scoreboard
            .teams
            .entry(key.into())
            .or_insert_with(|| ScoreTeam {
                parameters: TeamParameters {
                    display: Arc::new(json!({"text":key})),
                    options,
                    visibility,
                    collision: TeamCollision::Always,
                    formatting: 12,
                    prefix: Arc::new(json!({"text":"[team] "})),
                    suffix: Arc::new(json!({"text":" !"})),
                },
                members: BTreeSet::new(),
            })
            .members
            .insert(holder.into());
        scoreboard.membership.insert(holder.into(), key.into());
    }

    #[test]
    fn p1_nameplate_strict_64_and_living_discrete_32_ranges() {
        let scoreboard = ScoreboardState::default();
        let view = view(&scoreboard);
        let (generic_store, generic_models, mut generic) =
            fixture("minecraft:marker", &["net.minecraft.world.entity.Entity"]);
        visible_name(&mut generic);
        assert!(
            selected(
                &generic_store,
                &generic_models,
                &generic,
                &view,
                4096. - 0.001
            )
            .is_some()
        );
        for distance in [4096., 4096. + 0.001, f64::NAN] {
            assert!(selected(&generic_store, &generic_models, &generic, &view, distance).is_none());
        }
        let (store, models, mut cow) = fixture(
            "minecraft:cow",
            &[
                "net.minecraft.world.entity.Mob",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut cow);
        cow.metadata.insert(0, Metadata::Byte(2));
        assert!(
            selected(&store, &models, &cow, &view, 1024. - 0.001)
                .unwrap()
                .discrete
        );
        assert!(selected(&store, &models, &cow, &view, 1024.).is_none());
        cow.metadata.insert(0, Metadata::Byte(0));
        assert!(selected(&store, &models, &cow, &view, 1024.).is_some());
        // The 32-block check is LivingEntityRenderer-specific, not a shared flag gate.
        generic.metadata.insert(0, Metadata::Byte(2));
        assert!(
            selected(&generic_store, &generic_models, &generic, &view, 1024.)
                .unwrap()
                .discrete
        );
    }

    #[test]
    fn p1_nameplate_armor_stand_flag_only_bypasses_living_gates() {
        let scoreboard = ScoreboardState::default();
        let (store, models, mut entity) = fixture(
            "minecraft:armor_stand",
            &["net.minecraft.world.entity.LivingEntity"],
        );
        let mut view = view(&scoreboard);
        view.crosshair = Some(entity.handle);
        entity.metadata.insert(
            2,
            Metadata::OptionalComponent(Some(json!({"text":"Targeted"}))),
        );
        assert!(selected(&store, &models, &entity, &view, 1.).is_none());
        entity.metadata.insert(3, Metadata::Bool(true));
        entity.metadata.insert(0, Metadata::Byte(2 | 32));
        entity.metadata.insert(15, Metadata::Byte(16));
        entity.passenger_ids.push(88);
        view.render_names = false;
        view.camera_entity = Some(entity.handle.network_id);
        let descriptor = selected(&store, &models, &entity, &view, 4095.).unwrap();
        assert_eq!(descriptor.attachment, [0.; 3]);
        assert!(descriptor.discrete);
        entity.metadata.insert(2, Metadata::OptionalComponent(None));
        assert_eq!(
            selected(&store, &models, &entity, &view, 1.).unwrap().name,
            NameSource::Translation("minecraft:armor_stand")
        );
        assert!(selected(&store, &models, &entity, &view, 4096.).is_none());
    }

    #[test]
    fn p1_nameplate_generic_mob_avatar_and_living_dispatch() {
        let scoreboard = ScoreboardState::default();
        for (kind, classes) in [
            (
                "minecraft:marker",
                vec!["net.minecraft.world.entity.Entity"],
            ),
            (
                "minecraft:cow",
                vec![
                    "net.minecraft.world.entity.Mob",
                    "net.minecraft.world.entity.LivingEntity",
                ],
            ),
            (
                "minecraft:mannequin",
                vec![
                    "net.minecraft.world.entity.Avatar",
                    "net.minecraft.world.entity.LivingEntity",
                ],
            ),
        ] {
            let (store, models, mut entity) = fixture(kind, &classes);
            let mut view = view(&scoreboard);
            entity.metadata.insert(
                2,
                Metadata::OptionalComponent(Some(json!({"text":"Targeted"}))),
            );
            assert!(selected(&store, &models, &entity, &view, 1.).is_none());
            view.crosshair = Some(entity.handle);
            assert!(matches!(
                selected(&store, &models, &entity, &view, 1.).unwrap().name,
                NameSource::Component(_)
            ));
            view.crosshair = Some(Handle {
                generation: entity.handle.generation - 1,
                ..entity.handle
            });
            assert!(selected(&store, &models, &entity, &view, 1.).is_none());
            entity.metadata.insert(3, Metadata::Bool(true));
            assert!(selected(&store, &models, &entity, &view, 1.).is_some());
            entity
                .metadata
                .insert(2, Metadata::Component(json!({"text":"Wrong serializer"})));
            entity.metadata.insert(3, Metadata::Int(1));
            assert!(selected(&store, &models, &entity, &view, 1.).is_none());
        }
        let (store, models, entity) = fixture(
            "fixture:living",
            &["net.minecraft.world.entity.LivingEntity"],
        );
        assert_eq!(
            selected(&store, &models, &entity, &view(&scoreboard), 1.)
                .unwrap()
                .name,
            NameSource::Translation("fixture:living")
        );
        let (store, models, mut dragon) = fixture(
            "minecraft:ender_dragon",
            &[
                "net.minecraft.world.entity.Mob",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut dragon);
        dragon.metadata.insert(0, Metadata::Byte(2 | 32));
        let mut hidden = view(&scoreboard);
        hidden.render_names = false;
        assert!(selected(&store, &models, &dragon, &hidden, 2048.).is_some());
    }

    #[test]
    fn p1_nameplate_player_uses_profile_not_custom_or_tab_name_and_team_holder() {
        let (mut store, models, mut entity) = fixture(
            "minecraft:player",
            &[
                "net.minecraft.world.entity.Avatar",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut entity);
        let profile = PlayerProfile {
            uuid: entity.uuid.clone(),
            name: "ProfileName".into(),
            properties: Vec::new(),
            generation: 1,
            listed: false,
            latency: 0,
            game_mode: 0,
            display_name: Some(json!({"text":"Tab alias"})),
            list_order: 0,
            show_hat: true,
            chat_key: None,
            textures: None,
        };
        store.profiles.insert(entity.uuid.clone(), profile);
        let mut scoreboard = ScoreboardState::default();
        team(
            &mut scoreboard,
            "ProfileName",
            "names",
            TeamVisibility::Always,
            0,
        );
        let descriptor = selected(&store, &models, &entity, &view(&scoreboard), 1.).unwrap();
        assert_eq!(descriptor.name, NameSource::Player("ProfileName"));
        assert!(std::ptr::eq(
            descriptor.team.unwrap(),
            &scoreboard.teams["names"].parameters
        ));
        // Player.shouldShowName is true even with the custom-name flag off.
        entity.metadata.insert(3, Metadata::Bool(false));
        entity.metadata.insert(2, Metadata::OptionalComponent(None));
        assert!(selected(&store, &models, &entity, &view(&scoreboard), 1.).is_some());
    }

    #[test]
    fn p1_nameplate_team_visibility_spectator_and_friendly_invisible_matrix() {
        let (store, models, mut entity) = fixture(
            "minecraft:cow",
            &[
                "net.minecraft.world.entity.Mob",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut entity);
        for visibility in [
            TeamVisibility::Always,
            TeamVisibility::Never,
            TeamVisibility::HideForOtherTeams,
            TeamVisibility::HideForOwnTeam,
        ] {
            for viewer_membership in [None, Some("target"), Some("other")] {
                for invisible in [false, true] {
                    for friendly in [false, true] {
                        for spectator in [false, true] {
                            let mut scoreboard = ScoreboardState::default();
                            team(
                                &mut scoreboard,
                                &entity.uuid,
                                "target",
                                visibility,
                                if friendly { 2 } else { 0 },
                            );
                            if let Some(key) = viewer_membership {
                                team(&mut scoreboard, "Viewer", key, TeamVisibility::Always, 0);
                            }
                            entity
                                .metadata
                                .insert(0, Metadata::Byte(if invisible { 32 } else { 0 }));
                            let mut view = view(&scoreboard);
                            view.viewer_spectator = spectator;
                            let same = viewer_membership == Some("target");
                            let visible = spectator || !invisible || same && friendly;
                            let expected = match visibility {
                                TeamVisibility::Always => visible,
                                TeamVisibility::Never => false,
                                TeamVisibility::HideForOtherTeams => {
                                    viewer_membership.is_none() && visible
                                        || same && (friendly || visible)
                                }
                                TeamVisibility::HideForOwnTeam => !same && visible,
                            };
                            assert_eq!(
                                selected(&store, &models, &entity, &view, 1.).is_some(),
                                expected,
                                "{visibility:?} {viewer_membership:?} invisible={invisible} friendly={friendly} spectator={spectator}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn p1_nameplate_team_early_returns_and_local_player_default_gates() {
        let (mut store, models, mut entity) = fixture(
            "minecraft:cow",
            &[
                "net.minecraft.world.entity.Mob",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut entity);
        entity.passenger_ids.push(999); // Source isVehicle does not require a loaded passenger.
        let mut scoreboard = ScoreboardState::default();
        team(
            &mut scoreboard,
            &entity.uuid,
            "target",
            TeamVisibility::Always,
            0,
        );
        let mut hidden = view(&scoreboard);
        hidden.render_names = false;
        hidden.camera_entity = Some(entity.handle.network_id);
        assert!(selected(&store, &models, &entity, &hidden, 1.).is_some());
        entity.metadata.insert(0, Metadata::Byte(2));
        assert!(selected(&store, &models, &entity, &hidden, 1024.).is_none());
        entity.metadata.insert(0, Metadata::Byte(0));
        store.local_entity_id = Some(entity.handle.network_id);
        assert!(selected(&store, &models, &entity, &hidden, 1.).is_none());
        let empty = ScoreboardState::default();
        let mut ordinary = view(&empty);
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_none());
        entity.passenger_ids.clear();
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_some());
        ordinary.render_names = false;
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_none());
        ordinary.render_names = true;
        ordinary.camera_entity = Some(entity.handle.network_id);
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_none());
        ordinary.camera_entity = None;
        entity.metadata.insert(0, Metadata::Byte(32));
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_none());
        ordinary.viewer_spectator = true;
        assert!(selected(&store, &models, &entity, &ordinary, 1.).is_some());
    }

    #[test]
    fn p1_nameplate_attachment_uses_source_scale_and_interpolated_yaw() {
        let (store, mut models, mut entity) = fixture(
            "minecraft:cow",
            &[
                "net.minecraft.world.entity.Mob",
                "net.minecraft.world.entity.LivingEntity",
            ],
        );
        visible_name(&mut entity);
        models
            .schemas
            .get_mut(&entity.type_name)
            .unwrap()
            .dimensions
            .attachments
            .insert("NAME_TAG".into(), vec![[1., 2., 3.], [9.; 3]]);
        entity.attributes.insert(7, 2.);
        entity.transform.yaw = 180.;
        entity.previous.yaw = 0.;
        entity.physical.lerp_duration = 2;
        entity.physical.lerp_progress = 1;
        let scoreboard = ScoreboardState::default();
        // Shortest-arc interpolation is -90 degrees; NAME_TAG yRot negates it.
        let descriptor = nameplate(&store, &models, &entity, &view(&scoreboard), 1., 0, 0.)
            .unwrap()
            .unwrap();
        for (actual, expected) in descriptor.attachment.into_iter().zip([6., 4., -2.]) {
            assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
        }
        models
            .schemas
            .get_mut(&entity.type_name)
            .unwrap()
            .dimensions
            .attachments
            .remove("NAME_TAG");
        assert!(selected(&store, &models, &entity, &view(&scoreboard), 1.).is_none());
    }

    #[test]
    fn p1_nameplate_item_frame_uses_held_custom_name_and_written_title_override() {
        let (store, models, mut entity) = fixture(
            "minecraft:item_frame",
            &["net.minecraft.world.entity.Entity"],
        );
        visible_name(&mut entity); // Entity name/always flag must NOT trigger this override.
        let scoreboard = ScoreboardState::default();
        let mut view = view(&scoreboard);
        view.crosshair = Some(entity.handle);
        assert!(selected(&store, &models, &entity, &view, 1.).is_none());
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        Arc::make_mut(&mut stack.components).insert(
            "minecraft:custom_name".into(),
            json!({"text":"Held custom"}),
        );
        entity.metadata.insert(9, Metadata::Item(stack));
        assert_eq!(
            selected(&store, &models, &entity, &view, 1.)
                .unwrap()
                .attachment,
            [0., 1.75, f64::from(0.3_f32)],
        );
        entity.metadata.insert(8, Metadata::Direction(5));
        let descriptor = selected(&store, &models, &entity, &view, 1.).unwrap();
        let Metadata::Item(stack) = &entity.metadata[&9] else {
            unreachable!()
        };
        assert_eq!(
            descriptor.name,
            NameSource::Component(stack.component("minecraft:custom_name").unwrap())
        );
        assert_eq!(descriptor.team, None);
        assert_eq!(descriptor.attachment, [f64::from(0.3_f32), 1.75, 0.]);
        view.render_names = false;
        assert!(selected(&store, &models, &entity, &view, 1.).is_none());
        view.render_names = true;
        for (title, expected) in [
            ("Written title", true),
            (" \t\u{00a0}", false),
            ("\u{0085}", true),
        ] {
            let mut book = Stack::default();
            book.id = 2;
            book.count = 1;
            Arc::make_mut(&mut book.components).insert(
                "minecraft:written_book_content".into(),
                json!({"title":{"raw":title,"filtered":"Not used"}}),
            );
            entity.metadata.insert(9, Metadata::Item(book));
            assert_eq!(
                selected(&store, &models, &entity, &view, 1.).is_some(),
                expected
            );
            if expected {
                let descriptor = selected(&store, &models, &entity, &view, 1.).unwrap();
                let Metadata::Item(book) = &entity.metadata[&9] else {
                    unreachable!()
                };
                assert_eq!(
                    descriptor.name,
                    NameSource::Component(
                        &book.component("minecraft:written_book_content").unwrap()["title"]["raw"]
                    )
                );
            }
        }
    }
}
