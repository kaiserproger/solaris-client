//! Name-tag geometry from the local 26.1.2 NameTagFeatureRenderer$Storage,
//! PlayerTeam.getFormattedName and Camera.setRotation javap evidence.
use super::{
    Handle, PresentationStats, Store,
    nameplates::{self, NameSource, Nameplate, NameplateView},
};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content},
    ui::{
        ScoreboardState, TeamParameters,
        render::{TextRenderer, WorldTextOptions},
        shaping::TextLayout,
    },
};
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde_json::Value;
use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

#[derive(Default)]
pub(crate) struct Cache {
    layouts: BTreeMap<Handle, CachedLayout>,
    candidates: Vec<Candidate>,
    through_order: Vec<usize>,
}
struct CachedLayout {
    name: Identity,
    team: Option<Decoration>,
    font: usize,
    layout: TextLayout,
}
enum Identity {
    Component(Value),
    Player(String),
    Translation(String),
}
struct Decoration {
    formatting: u8,
    prefix: Arc<Value>,
    suffix: Arc<Value>,
}
struct Candidate {
    handle: Handle,
    distance_squared: f64,
    transform: Mat4,
    discrete: bool,
    light: [f32; 2],
    submitted: bool,
}
impl Identity {
    fn matches(&self, name: NameSource<'_>) -> bool {
        match (self, name) {
            (Self::Component(cached), NameSource::Component(received)) => cached == received,
            (Self::Player(cached), NameSource::Player(received))
            | (Self::Translation(cached), NameSource::Translation(received)) => cached == received,
            _ => false,
        }
    }
    fn new(name: NameSource<'_>) -> Self {
        match name {
            NameSource::Component(component) => Self::Component(component.clone()),
            NameSource::Player(name) => Self::Player(name.into()),
            NameSource::Translation(kind) => Self::Translation(kind.into()),
        }
    }
}
impl Decoration {
    fn matches(&self, team: &TeamParameters) -> bool {
        self.formatting == team.formatting
            && self.prefix.as_ref() == team.prefix.as_ref()
            && self.suffix.as_ref() == team.suffix.as_ref()
    }
    fn new(team: &TeamParameters) -> Self {
        Self {
            formatting: team.formatting,
            prefix: Arc::clone(&team.prefix),
            suffix: Arc::clone(&team.suffix),
        }
    }
}
impl Cache {
    fn prepare(
        &mut self,
        handle: Handle,
        plate: &Nameplate<'_>,
        holder: &str,
        board: &ScoreboardState,
        text: &TextRenderer,
    ) -> Result<()> {
        let font = text as *const TextRenderer as usize;
        let dirty = self.layouts.get(&handle).is_none_or(|cached| {
            cached.font != font
                || !cached.name.matches(plate.name)
                || match (&cached.team, plate.team) {
                    (Some(cached), Some(team)) => !cached.matches(team),
                    (None, None) => false,
                    _ => true,
                }
        });
        if dirty {
            let component = match plate.name {
                NameSource::Component(component) => Cow::Borrowed(component),
                NameSource::Player(name) => Cow::Owned(serde_json::json!({"text": name})),
                NameSource::Translation(kind) => {
                    let (namespace, path) = kind.split_once(':').unwrap_or(("minecraft", kind));
                    Cow::Owned(
                        serde_json::json!({"translate": format!("entity.{namespace}.{}", path.replace('/', "."))}),
                    )
                }
            };
            // Reuse the scoreboard's prefix/name/suffix and inherited color
            // convention. These owned components are created only on dirtiness.
            let layout = if let Some(team) = plate.team {
                let component = Arc::new(component.into_owned());
                let mut component = board.formatted_name(holder, Some(&component));
                // PlayerTeam also accepts the non-color ChatFormatting ordinals;
                // RESET leaves the empty root style alone, preserving child styles.
                let flag = match team.formatting {
                    16 => Some("obfuscated"),
                    17 => Some("bold"),
                    18 => Some("strikethrough"),
                    19 => Some("underlined"),
                    20 => Some("italic"),
                    _ => None,
                };
                if let Some(flag) = flag {
                    Arc::make_mut(&mut component)[flag] = Value::Bool(true);
                }
                text.layout(&component, f32::MAX)?
            } else {
                text.layout(component.as_ref(), f32::MAX)?
            };
            self.layouts.insert(
                handle,
                CachedLayout {
                    name: Identity::new(plate.name),
                    team: plate.team.map(Decoration::new),
                    font,
                    layout,
                },
            );
        }
        Ok(())
    }
}

fn billboard(
    origin: [f64; 3],
    position: [f64; 3],
    attachment: [f64; 3],
    rotation: [f32; 2],
) -> Mat4 {
    let translation = Vec3::new(
        (position[0] + attachment[0] - origin[0]) as f32,
        (position[1] + attachment[1] + 0.5 - origin[1]) as f32,
        (position[2] + attachment[2] - origin[2]) as f32,
    );
    let orientation = Quat::from_euler(
        EulerRot::YXZ,
        std::f32::consts::PI - rotation[0].to_radians(),
        -rotation[1].to_radians(),
        0.,
    );
    Mat4::from_scale_rotation_translation(Vec3::new(0.025, -0.025, 0.025), orientation, translation)
}
fn append_pass(
    content: &Content,
    text: &TextRenderer,
    layout: &TextLayout,
    candidate: &Candidate,
    background: Option<u32>,
    through: bool,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let bright = !candidate.discrete && !through;
    text.append_world_layout(
        content,
        layout,
        candidate.transform * Mat4::from_translation(Vec3::new(-layout.width.ceil() / 2., 0., 0.)),
        if bright { 0xffffffff } else { 0x80ffffff },
        &WorldTextOptions {
            // LightCoordsUtil.lightCoordsWithEmission raises both received
            // channels to emission2; it does not replace them with fullbright.
            light: if bright {
                candidate.light.map(|level| level.max(2.))
            } else {
                candidate.light
            },
            layer: if through {
                EntityMaterial::TextSeeThrough
            } else {
                EntityMaterial::Text
            },
            background: if bright { None } else { background },
            ..Default::default()
        },
        out,
    )
}
impl Store {
    pub(crate) fn append_nameplate_geometry(
        &mut self,
        content: &Content,
        origin: [f64; 3],
        camera: [f64; 3],
        camera_rotation: [f32; 2],
        radius: f32,
        tick: u64,
        partial: f32,
        view: &NameplateView<'_>,
        light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
        out: &mut DynamicGeometry,
    ) -> Result<PresentationStats> {
        if !radius.is_finite() || radius <= 0. || radius > 4096. {
            return Err("entity presentation radius bound".into());
        }
        // Taking the cache, rather than cloning the store/text/components,
        // permits policy to borrow received state while layouts are updated.
        // Restore it on errors as well so the reusable candidate capacity survives.
        let mut cache = std::mem::take(&mut self.nameplate_cache);
        let result = (|| {
            cache.layouts.retain(|handle, _| {
                self.ids
                    .get(&handle.network_id)
                    .is_some_and(|slot| self.slots[*slot].handle == *handle)
            });
            cache.candidates.clear();
            cache.through_order.clear();
            let mut stats = PresentationStats {
                state_revision: self.revision.to_string(),
                known: self.len(),
                ..Default::default()
            };
            let Some(text) = self.text.as_deref().filter(|text| text.can_render()) else {
                return Ok(stats);
            };
            let models = self
                .models
                .as_deref()
                .ok_or("entity model cache not activated")?;
            let range = f64::from(radius.min(64.));
            let query_radius = range + self.query_padding();
            let min = [
                ((camera[0] - query_radius) / 16.).floor() as i32,
                ((camera[2] - query_radius) / 16.).floor() as i32,
            ];
            let max = [
                ((camera[0] + query_radius) / 16.).floor() as i32,
                ((camera[2] + query_radius) / 16.).floor() as i32,
            ];
            // Body submission skips invisible/marker entities and clears its ID
            // scratch vector; query the same spatial index independently.
            for (_, bucket) in self
                .spatial
                .range([min[0], i32::MIN]..=[max[0], i32::MAX])
                .filter(|(cell, _)| cell[1] >= min[1] && cell[1] <= max[1])
            {
                for id in bucket {
                    let entity = &self.slots[self.ids[id]];
                    let position = entity.presented_transform(tick, partial).position;
                    let distance_squared = position
                        .iter()
                        .zip(camera)
                        .map(|(a, b)| (a - b) * (a - b))
                        .sum::<f64>();
                    if distance_squared > range * range {
                        continue;
                    }
                    stats.eligible += 1;
                    let plate = match nameplates::nameplate(
                        self,
                        models,
                        entity,
                        view,
                        distance_squared,
                        tick,
                        partial,
                    ) {
                        Ok(Some(plate)) => plate,
                        Ok(None) => continue,
                        Err(reason) => {
                            stats.unrendered.insert(entity.type_name.clone(), reason);
                            continue;
                        }
                    };
                    let (dimensions, scale) = models.dimensions(entity)?;
                    let Some(received) = super::presentation::entity_light(
                        entity, models, tick, partial, dimensions, scale, light,
                    )?
                    else {
                        stats.lighting_pending += 1;
                        continue;
                    };
                    let holder = match plate.name {
                        NameSource::Player(name) => name,
                        _ => entity.uuid.as_str(),
                    };
                    cache.prepare(entity.handle, &plate, holder, view.scoreboard, text)?;
                    cache.candidates.push(Candidate {
                        handle: entity.handle,
                        distance_squared,
                        transform: billboard(origin, position, plate.attachment, camera_rotation),
                        discrete: plate.discrete,
                        light: received,
                        submitted: false,
                    });
                }
            }
            // Source sorts only SEE_THROUGH far-to-near; NORMAL retains
            // extraction order. Both scratch vectors retain their capacity.
            cache.through_order.extend(
                cache
                    .candidates
                    .iter()
                    .enumerate()
                    .filter_map(|(index, candidate)| (!candidate.discrete).then_some(index)),
            );
            let candidates = &cache.candidates;
            cache.through_order.sort_unstable_by(|&a, &b| {
                candidates[b]
                    .distance_squared
                    .total_cmp(&candidates[a].distance_squared)
                    .then_with(|| a.cmp(&b))
            });
            let alpha = (view.background_opacity * 255.) as u32;
            let background = (alpha != 0).then_some(alpha << 24);
            let start = out.vertices.len();
            for &index in &cache.through_order {
                let candidate = &mut cache.candidates[index];
                let before = out.vertices.len();
                append_pass(
                    content,
                    text,
                    &cache.layouts[&candidate.handle].layout,
                    candidate,
                    background,
                    true,
                    out,
                )?;
                candidate.submitted = out.vertices.len() > before;
            }
            for candidate in &cache.candidates {
                let before = out.vertices.len();
                append_pass(
                    content,
                    text,
                    &cache.layouts[&candidate.handle].layout,
                    candidate,
                    background,
                    false,
                    out,
                )?;
                if candidate.submitted || out.vertices.len() > before {
                    stats.submitted += 1;
                }
            }
            stats.vertices = out.vertices.len() - start;
            Ok(stats)
        })();
        self.nameplate_cache = cache;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::Vertex,
        entities::{Entity, Metadata, Models, Transform, resources::EntitySchema},
        ui::{ScoreTeam, TeamCollision, TeamVisibility},
    };
    use std::collections::BTreeSet;

    fn fixture() -> (Content, Store) {
        let mut content = Content::test_cube();
        // Authored empty-provider fonts paint the real missing-glyph quads;
        // no proprietary bitmap, archive or decompiled implementation is tracked.
        content.insert_test_font("minecraft:default", &serde_json::json!({"providers":[]}));
        content.insert_test_resource(
            "assets/minecraft/lang/en_us.json",
            br#"{"entity.minecraft.marker":"Marker"}"#,
        );
        let schema: EntitySchema = serde_json::from_value(serde_json::json!({
            "protocol_id":0, "width":1., "height":2., "eye_height":1.,
            "fire_immune":false, "class":"net.minecraft.world.entity.Marker",
            "hierarchy":["net.minecraft.world.entity.Entity"],
            "dimension_constants":{}, "enum_ids":{}, "enum_numbers":{},
            "method_owners":{}, "default_attributes":{},
            "accessors":{
                "DATA_SHARED_FLAGS_ID":[0,0],
                "DATA_CUSTOM_NAME":[2,6],
                "DATA_CUSTOM_NAME_VISIBLE":[3,8]
            },
            "dimensions":{
                "width":1., "height":2., "eye_height":1., "fixed":false,
                "attachments":{"NAME_TAG":[[0.25,1.5,0.5]]}
            }
        }))
        .unwrap();
        let mut store = Store::new();
        store.set_text_renderer(Arc::new(TextRenderer::load(&content).unwrap()));
        store.activate_models(Arc::new(Models {
            rigs: BTreeMap::new(),
            schemas: BTreeMap::from([("minecraft:marker".into(), schema)]),
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
        }));
        (content, store)
    }

    fn insert(
        store: &mut Store,
        id: i32,
        position: [f64; 3],
        yaw: f32,
        name: &str,
        flags: i8,
    ) -> Handle {
        let transform = Transform {
            position,
            yaw,
            pitch: 0.,
            head_yaw: yaw,
        };
        store.generation += 1;
        let handle = Handle {
            network_id: id,
            generation: store.generation,
        };
        store.ids.insert(id, store.slots.len());
        store
            .spatial
            .entry(super::super::cell(position))
            .or_default()
            .insert(id);
        store.slots.push(Entity {
            handle,
            uuid: format!("owned-nameplate-{id}"),
            kind: 0,
            type_name: "minecraft:marker".into(),
            position,
            velocity: [0.; 3],
            transform,
            previous: transform,
            on_ground: false,
            spawn_data: 0,
            observed_tick: 0,
            revision: 0,
            appearance_revision: 0,
            physical: Default::default(),
            binding: None,
            source: None,
            minecart_steps: Vec::new(),
            acceleration_power: None,
            metadata: BTreeMap::from([
                (0, Metadata::Byte(flags)),
                (
                    2,
                    Metadata::OptionalComponent(Some(serde_json::json!({"text":name}))),
                ),
                (3, Metadata::Bool(true)),
            ]),
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
        });
        handle
    }
    fn view(board: &ScoreboardState, background_opacity: f32) -> NameplateView<'_> {
        NameplateView {
            crosshair: None,
            camera_entity: None,
            viewer_name: None,
            viewer_spectator: false,
            render_names: true,
            scoreboard: board,
            background_opacity,
        }
    }
    fn body() -> DynamicGeometry {
        let mut out = DynamicGeometry::default();
        out.vertices.push(Vertex {
            position: [-90.; 3],
            color: [1.; 3],
            uv: [0.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            alpha: 1.,
            texture_layer: 0,
            light: [7., 3.],
            normal: [0.; 3],
        });
        out.indices[EntityMaterial::EntityCutout as usize].extend([0, 0, 0]);
        out
    }
    fn draw(
        store: &mut Store,
        content: &Content,
        board: &ScoreboardState,
        opacity: f32,
    ) -> DynamicGeometry {
        let mut out = DynamicGeometry::default();
        store
            .append_nameplate_geometry(
                content,
                [12., 20., 30.],
                [12., 20., 28.],
                [0., 0.],
                64.,
                0,
                0.,
                &view(board, opacity),
                &|_| Ok(Some([0., 7.])),
                &mut out,
            )
            .unwrap();
        out
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.00001, "{a} != {b}");
    }

    #[test]
    fn p1_nameplate_real_passes_received_light_emission_background_and_body() {
        let (content, mut store) = fixture();
        // A named invisible marker has no submitted body but still has a tag.
        let handle = insert(&mut store, 7, [12., 20., 30.], 0., "A", 32);
        let board = ScoreboardState::default();
        let mut out = body();
        let stats = store
            .append_nameplate_geometry(
                &content,
                [12., 20., 30.],
                [12., 20., 28.],
                [0., 0.],
                64.,
                0,
                0.,
                &view(&board, 0.6),
                &|point| {
                    assert_eq!(point, [12., 21., 30.]);
                    Ok(Some([0., 7.]))
                },
                &mut out,
            )
            .unwrap();
        assert_eq!(stats.submitted, 1);
        assert!(stats.unrendered.is_empty());
        assert_eq!(stats.vertices, out.vertices.len() - 1);
        assert_eq!(out.vertices[0].position, [-90.; 3]);
        assert_eq!(
            out.indices[EntityMaterial::EntityCutout as usize],
            [0, 0, 0]
        );
        let normal = &out.indices[EntityMaterial::Text as usize];
        let through = &out.indices[EntityMaterial::TextSeeThrough as usize];
        assert!(!normal.is_empty() && !through.is_empty());
        assert!(
            normal
                .iter()
                .all(|&i| out.vertices[i as usize].light == [2., 7.]
                    && out.vertices[i as usize].alpha == 1.)
        );
        assert!(
            through
                .iter()
                .all(|&i| i > 0 && out.vertices[i as usize].light == [0., 7.])
        );
        assert!(through.iter().all(|&i| i < normal[0]));
        // The actual first quad is the received-options background, not a fake
        // glyph/material record. NORMAL has no background for ordinary names.
        for vertex in &out.vertices[1..5] {
            assert_eq!(vertex.color, [0.; 3]);
            close(vertex.alpha, 153. / 255.);
        }
        let dim = &out.vertices[5..normal[0] as usize];
        assert!(!dim.is_empty());
        assert!(
            dim.iter()
                .all(|v| v.color == [1.; 3] && v.alpha == 128. / 255.)
        );
        let width = store.nameplate_cache.layouts[&handle].layout.width;
        close(
            out.vertices[1].position[0],
            0.25 + (width.ceil() / 2. + 1.) * 0.025,
        );
        close(out.vertices[1].position[1], 2.025); // attachment Y + source .5 + top border
        close(out.vertices[1].position[2], 0.50025); // background depth after billboard scale
        close(out.vertices[2].position[1], 1.8);

        store
            .get_mut(7)
            .unwrap()
            .metadata
            .insert(0, Metadata::Byte(2));
        let discrete = draw(&mut store, &content, &board, 0.6);
        assert!(discrete.indices[EntityMaterial::TextSeeThrough as usize].is_empty());
        assert!(!discrete.indices[EntityMaterial::Text as usize].is_empty());
        assert!(discrete.vertices.iter().all(|v| v.light == [0., 7.]));
        assert!(
            discrete.vertices[4..]
                .iter()
                .all(|v| v.alpha == 128. / 255.)
        );
        let without_background = draw(&mut store, &content, &board, 0.);
        assert_eq!(
            without_background.vertices.len() + 4,
            discrete.vertices.len()
        );
        assert!(
            without_background
                .vertices
                .iter()
                .all(|v| v.color == [1.; 3])
        );
    }

    #[test]
    fn p1_nameplate_camera_winding_yaw_attachment_and_world_scale() {
        let (content, mut store) = fixture();
        insert(&mut store, 7, [12., 20., 30.], 90., "A", 0);
        let board = ScoreboardState::default();
        for rotation in [[0., 0.], [90., 0.], [125., 35.], [-40., -25.]] {
            let mut out = DynamicGeometry::default();
            store
                .append_nameplate_geometry(
                    &content,
                    [12., 20., 30.],
                    [12., 20., 28.],
                    rotation,
                    64.,
                    0,
                    0.,
                    &view(&board, 0.25),
                    &|_| Ok(Some([3., 1.])),
                    &mut out,
                )
                .unwrap();
            let (sy, cy) = rotation[0].to_radians().sin_cos();
            let (sp, cp) = rotation[1].to_radians().sin_cos();
            let facing_camera = Vec3::new(sy * cp, sp, -cy * cp);
            for material in [EntityMaterial::Text, EntityMaterial::TextSeeThrough] {
                for triangle in out.indices[material as usize].chunks_exact(3) {
                    let point = |i: u32| Vec3::from_array(out.vertices[i as usize].position);
                    let [a, b, c] = [point(triangle[0]), point(triangle[1]), point(triangle[2])];
                    assert!((b - a).cross(c - a).dot(facing_camera) > 0.);
                }
            }
            let a = Vec3::from_array(out.vertices[0].position);
            let b = Vec3::from_array(out.vertices[1].position);
            // Real background TL->BL spans 9 text units at .025 world scale.
            close((b - a).length(), 9. * 0.025);
            if rotation == [0., 0.] {
                let c = Vec3::from_array(out.vertices[2].position);
                // NAME_TAG(.25,1.5,.5) rotated by -yaw90 gives (-.5,1.5,.25).
                close((a.x + c.x) / 2., -0.5);
                close(a.y, 2.025);
                close(a.z, 0.25025);
            }
        }
    }

    #[test]
    fn p1_nameplate_cache_reuses_layout_replaces_names_and_prunes_generations() {
        let (content, mut store) = fixture();
        let board = ScoreboardState::default();
        let handle = insert(&mut store, 7, [12., 20., 30.], 0., "A", 0);
        let first = draw(&mut store, &content, &board, 0.25);
        let glyphs = store.nameplate_cache.layouts[&handle]
            .layout
            .glyphs
            .as_ptr();
        let scratch = store.nameplate_cache.candidates.as_ptr();
        let repeated = draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle]
                .layout
                .glyphs
                .as_ptr(),
            glyphs
        );
        assert_eq!(store.nameplate_cache.candidates.as_ptr(), scratch);
        assert_eq!(first.vertices.len(), repeated.vertices.len());
        store.get_mut(7).unwrap().metadata.insert(
            2,
            Metadata::OptionalComponent(Some(serde_json::json!({"text":"BBB"}))),
        );
        let renamed = draw(&mut store, &content, &board, 0.25);
        assert!(renamed.vertices.len() > first.vertices.len());
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            3
        );
        assert!(matches!(&store.nameplate_cache.layouts[&handle].name,
            Identity::Component(value) if value["text"] == "BBB"));
        store.remove(7);
        let replacement = insert(&mut store, 7, [12., 20., 30.], 0., "C", 0);
        assert_ne!(handle, replacement);
        let replaced = draw(&mut store, &content, &board, 0.25);
        assert_eq!(replaced.vertices.len(), first.vertices.len());
        assert!(!store.nameplate_cache.layouts.contains_key(&handle));
        assert!(store.nameplate_cache.layouts.contains_key(&replacement));
        store.remove(7);
        assert!(draw(&mut store, &content, &board, 0.25).vertices.is_empty());
        assert!(store.nameplate_cache.layouts.is_empty());
    }

    #[test]
    fn p1_nameplate_team_decoration_cache_uses_only_rendered_identity() {
        let (content, mut store) = fixture();
        let handle = insert(&mut store, 7, [12., 20., 30.], 0., "A", 0);
        let holder = store.get(7).unwrap().uuid.clone();
        let mut board = ScoreboardState::default();
        board.membership.insert(holder.clone(), "owned".into());
        board.teams.insert(
            "owned".into(),
            ScoreTeam {
                members: BTreeSet::from([holder]),
                parameters: TeamParameters {
                    display: Arc::new(serde_json::json!({"text":"unused display"})),
                    options: 0,
                    visibility: TeamVisibility::Always,
                    collision: TeamCollision::Always,
                    formatting: 12,
                    prefix: Arc::new(serde_json::json!({"text":"[","color":"blue"})),
                    suffix: Arc::new(serde_json::json!({"text":"]"})),
                },
            },
        );
        let initial = draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            3
        );
        assert!(
            initial
                .vertices
                .iter()
                .any(|v| v.color == [1., 85. / 255., 85. / 255.])
        );
        assert!(
            initial
                .vertices
                .iter()
                .any(|v| v.color == [85. / 255., 85. / 255., 1.])
        );
        let glyphs = store.nameplate_cache.layouts[&handle]
            .layout
            .glyphs
            .as_ptr();
        let team = &mut board.teams.get_mut("owned").unwrap().parameters;
        team.display = Arc::new(serde_json::json!({"text":"not in a name tag"}));
        team.options = 1;
        team.collision = TeamCollision::Never;
        draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle]
                .layout
                .glyphs
                .as_ptr(),
            glyphs
        );
        let team = &mut board.teams.get_mut("owned").unwrap().parameters;
        team.prefix = Arc::new(serde_json::json!({"text":"LONG"}));
        team.formatting = 10;
        let changed = draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            6
        );
        assert!(changed.vertices.len() > initial.vertices.len());
        assert!(
            changed
                .vertices
                .iter()
                .any(|v| v.color == [85. / 255., 1., 85. / 255.])
        );
        board.membership.clear();
        let removed = draw(&mut store, &content, &board, 0.25);
        assert!(store.nameplate_cache.layouts[&handle].team.is_none());
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            1
        );
        assert!(
            removed
                .vertices
                .iter()
                .all(|v| v.color == [0.; 3] || v.color == [1.; 3])
        );
    }

    #[test]
    fn p1_nameplate_received_light_pending_range_and_seethrough_distance_order() {
        let (content, mut store) = fixture();
        insert(&mut store, 1, [12., 20., 30.], 0., "near", 0);
        insert(&mut store, 2, [12., 20., 35.], 0., "far", 0);
        insert(&mut store, 3, [12., 20., 92.], 0., "strict64", 0);
        let board = ScoreboardState::default();
        let out = draw(&mut store, &content, &board, 0.25);
        assert_eq!(store.nameplate_cache.candidates.len(), 2);
        assert_eq!(
            store.nameplate_cache.candidates[store.nameplate_cache.through_order[0]]
                .handle
                .network_id,
            2
        );
        close(out.vertices[0].position[2], 5.50025);
        let mut bounded = DynamicGeometry::default();
        let stats = store
            .append_nameplate_geometry(
                &content,
                [12., 20., 30.],
                [12., 20., 28.],
                [0., 0.],
                4.,
                0,
                0.,
                &view(&board, 0.25),
                &|_| Ok(Some([7., 3.])),
                &mut bounded,
            )
            .unwrap();
        assert_eq!(stats.submitted, 1);
        let mut pending = body();
        let stats = store
            .append_nameplate_geometry(
                &content,
                [12., 20., 30.],
                [12., 20., 28.],
                [0., 0.],
                64.,
                0,
                0.,
                &view(&board, 0.25),
                &|_| Ok(None),
                &mut pending,
            )
            .unwrap();
        assert_eq!(stats.lighting_pending, 2);
        assert_eq!(stats.submitted, 0);
        assert_eq!(stats.vertices, 0);
        assert_eq!(pending.vertices.len(), 1);
    }

    #[test]
    fn p1_nameplate_translation_font_reset_and_player_layout_identity() {
        let (mut content, mut store) = fixture();
        let board = ScoreboardState::default();
        let handle = insert(&mut store, 7, [12., 20., 30.], 0., "unused", 0);
        store
            .get_mut(7)
            .unwrap()
            .metadata
            .insert(2, Metadata::OptionalComponent(None));
        let translated = draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            6
        );
        assert!(matches!(&store.nameplate_cache.layouts[&handle].name,
            Identity::Translation(kind) if kind == "minecraft:marker"));
        content.insert_test_resource(
            "assets/minecraft/lang/en_us.json",
            br#"{"entity.minecraft.marker":"Reloaded marker"}"#,
        );
        store.set_text_renderer(Arc::new(TextRenderer::load(&content).unwrap()));
        assert!(store.nameplate_cache.layouts.is_empty());
        assert!(store.nameplate_cache.candidates.is_empty());
        let reloaded = draw(&mut store, &content, &board, 0.25);
        assert_eq!(
            store.nameplate_cache.layouts[&handle].layout.glyphs.len(),
            15
        );
        assert!(reloaded.vertices.len() > translated.vertices.len());

        let text = store.text.as_deref().unwrap();
        let mut cache = Cache::default();
        let mut plate = Nameplate {
            name: NameSource::Player("Profile"),
            team: None,
            attachment: [0.; 3],
            discrete: false,
        };
        cache
            .prepare(handle, &plate, "Profile", &board, text)
            .unwrap();
        let glyphs = cache.layouts[&handle].layout.glyphs.as_ptr();
        cache
            .prepare(handle, &plate, "Profile", &board, text)
            .unwrap();
        assert_eq!(cache.layouts[&handle].layout.glyphs.as_ptr(), glyphs);
        let candidate = Candidate {
            handle,
            distance_squared: 1.,
            transform: Mat4::IDENTITY,
            discrete: false,
            light: [3., 0.],
            submitted: false,
        };
        let mut original = DynamicGeometry::default();
        append_pass(
            &content,
            text,
            &cache.layouts[&handle].layout,
            &candidate,
            None,
            false,
            &mut original,
        )
        .unwrap();
        plate.name = NameSource::Player("NewProfile");
        cache
            .prepare(handle, &plate, "NewProfile", &board, text)
            .unwrap();
        let mut renamed = DynamicGeometry::default();
        append_pass(
            &content,
            text,
            &cache.layouts[&handle].layout,
            &candidate,
            None,
            false,
            &mut renamed,
        )
        .unwrap();
        assert!(matches!(&cache.layouts[&handle].name,
            Identity::Player(name) if name == "NewProfile"));
        assert_eq!(cache.layouts[&handle].layout.glyphs.len(), 10);
        assert!(renamed.vertices.len() > original.vertices.len());
        assert!(renamed.indices[EntityMaterial::TextSeeThrough as usize].is_empty());
        assert!(
            renamed
                .vertices
                .iter()
                .all(|vertex| vertex.light == [3., 2.])
        );
    }
}
