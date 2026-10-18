//! Client display render snapshots: received data marks dirty fields; Source entity
//! ticks commit them, and transformation interpolation samples the last rendered pose.
use super::{Entity, Metadata, Models, physical};
use glam::{Mat4, Quat, Vec3};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug)]
pub(crate) struct Transformation {
    translation: Vec3,
    scale: Vec3,
    left: Quat,
    right: Quat,
}
impl Default for Transformation {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            scale: Vec3::ONE,
            left: Quat::IDENTITY,
            right: Quat::IDENTITY,
        }
    }
}
impl Transformation {
    fn received(models: &Models, e: &Entity) -> Self {
        let vector = |name, default| match physical::value(models, e, name) {
            Some(Metadata::Vector(v)) => Vec3::from_array(*v),
            _ => default,
        };
        let rotation = |name| match physical::value(models, e, name) {
            Some(Metadata::Quaternion(v)) => Quat::from_array(*v),
            _ => Quat::IDENTITY,
        };
        Self {
            translation: vector("DATA_TRANSLATION_ID", Vec3::ZERO),
            scale: vector("DATA_SCALE_ID", Vec3::ONE),
            left: rotation("DATA_LEFT_ROTATION_ID"),
            right: rotation("DATA_RIGHT_ROTATION_ID"),
        }
    }
    fn interpolate(self, target: Self, t: f32) -> Self {
        Self {
            translation: self.translation + (target.translation - self.translation) * t,
            scale: self.scale + (target.scale - self.scale) * t,
            left: self.left.slerp(target.left, t),
            right: self.right.slerp(target.right, t),
        }
    }
    fn matrix(self) -> Mat4 {
        Mat4::from_translation(self.translation)
            * Mat4::from_quat(self.left)
            * Mat4::from_scale(self.scale)
            * Mat4::from_quat(self.right)
    }
}
#[derive(Debug)]
pub(crate) struct State {
    pub age: i32,
    start_tick: i64,
    duration: i32,
    pub last_progress: f32,
    pub ready: bool,
    dirty: bool,
    update_start: bool,
    update_duration: bool,
    from: Transformation,
    to: Transformation,
    pub billboard: i32,
    pub brightness: i32,
    pub shadow_from: [f32; 2],
    pub shadow_to: [f32; 2],
    pub glow_color: i32,
    pub background_from: i32,
    pub background_to: i32,
    pub opacity_from: i32,
    pub opacity_to: i32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            age: 0,
            start_tick: i64::from(i32::MIN),
            duration: 0,
            last_progress: 0.,
            ready: false,
            dirty: false,
            update_start: false,
            update_duration: false,
            from: Transformation::default(),
            to: Transformation::default(),
            billboard: 0,
            brightness: -1,
            shadow_from: [0., 1.],
            shadow_to: [0., 1.],
            glow_color: -1,
            background_from: 0x40000000,
            background_to: 0x40000000,
            opacity_from: -1,
            opacity_to: -1,
        }
    }
}
pub(crate) fn is_display(e: &Entity) -> bool {
    matches!(
        e.type_name.as_str(),
        "minecraft:block_display" | "minecraft:item_display" | "minecraft:text_display"
    )
}
pub(crate) fn on_metadata(models: &Models, e: &mut Entity, received: &BTreeMap<u8, Metadata>) {
    if !is_display(e) {
        return;
    }
    let changed = |name| {
        models
            .accessor(&e.type_name, name)
            .is_some_and(|id| received.contains_key(&id))
    };
    let start = changed("DATA_TRANSFORMATION_INTERPOLATION_START_DELTA_TICKS_ID");
    let duration = changed("DATA_TRANSFORMATION_INTERPOLATION_DURATION_ID");
    let dirty = [
        "DATA_TRANSLATION_ID",
        "DATA_SCALE_ID",
        "DATA_LEFT_ROTATION_ID",
        "DATA_RIGHT_ROTATION_ID",
        "DATA_BILLBOARD_RENDER_CONSTRAINTS_ID",
        "DATA_BRIGHTNESS_OVERRIDE_ID",
        "DATA_SHADOW_RADIUS_ID",
        "DATA_SHADOW_STRENGTH_ID",
        "DATA_BLOCK_STATE_ID",
        "DATA_ITEM_STACK_ID",
        "DATA_ITEM_DISPLAY_ID",
        "DATA_TEXT_ID",
        "DATA_LINE_WIDTH_ID",
        "DATA_BACKGROUND_COLOR_ID",
        "DATA_TEXT_OPACITY_ID",
        "DATA_STYLE_FLAGS_ID",
    ]
    .iter()
    .any(|name| changed(name));
    let movement = models
        .accessor(&e.type_name, "DATA_POS_ROT_INTERPOLATION_DURATION_ID")
        .and_then(|id| received.get(&id))
        .and_then(Metadata::int);
    if let Some(duration) = movement {
        e.physical.lerp_duration = duration.clamp(0, 59) as u8;
    }
    let state = e
        .physical
        .display
        .get_or_insert_with(|| Box::new(State::default()));
    state.update_start |= start;
    state.update_duration |= duration;
    state.dirty |= dirty;
}
fn source_float(models: &Models, e: &Entity, name: &str, default: f32) -> f32 {
    match physical::value(models, e, name) {
        Some(Metadata::Float(v)) => *v,
        _ => default,
    }
}
pub(crate) fn tick(models: &Models, e: &mut Entity) {
    let mut state = e
        .physical
        .display
        .take()
        .unwrap_or_else(|| Box::new(State::default()));
    state.age = state.age.wrapping_add(1);
    if state.update_start {
        state.update_start = false;
        state.start_tick = i64::from(state.age.wrapping_add(physical::number(
            models,
            e,
            "DATA_TRANSFORMATION_INTERPOLATION_START_DELTA_TICKS_ID",
            0,
        )));
    }
    if state.update_duration {
        state.update_duration = false;
        state.duration = physical::number(
            models,
            e,
            "DATA_TRANSFORMATION_INTERPOLATION_DURATION_ID",
            0,
        );
    }
    if state.dirty {
        state.dirty = false;
        let interpolate = state.duration != 0 && state.ready;
        let transform = Transformation::received(models, e);
        state.from = if interpolate {
            state.from.interpolate(state.to, state.last_progress)
        } else {
            transform
        };
        state.to = transform;
        let shadow = [
            source_float(models, e, "DATA_SHADOW_RADIUS_ID", 0.),
            source_float(models, e, "DATA_SHADOW_STRENGTH_ID", 1.),
        ];
        state.shadow_from = if interpolate {
            std::array::from_fn(|i| {
                state.shadow_from[i]
                    + (state.shadow_to[i] - state.shadow_from[i]) * state.last_progress
            })
        } else {
            shadow
        };
        state.shadow_to = shadow;
        let background = physical::number(models, e, "DATA_BACKGROUND_COLOR_ID", 0x40000000);
        let opacity = physical::number(models, e, "DATA_TEXT_OPACITY_ID", -1);
        state.background_from = if interpolate {
            argb_lerp(
                state.background_from,
                state.background_to,
                state.last_progress,
            )
        } else {
            background
        };
        state.background_to = background;
        state.opacity_from = if interpolate {
            int_lerp(state.opacity_from, state.opacity_to, state.last_progress)
        } else {
            opacity
        };
        state.opacity_to = opacity;
        state.billboard = physical::number(models, e, "DATA_BILLBOARD_RENDER_CONSTRAINTS_ID", 0);
        state.brightness = physical::number(models, e, "DATA_BRIGHTNESS_OVERRIDE_ID", -1);
        state.glow_color = physical::number(models, e, "DATA_GLOW_COLOR_OVERRIDE_ID", -1);
        state.ready = true;
    }
    e.physical.display = Some(state);
}
fn int_lerp(a: i32, b: i32, t: f32) -> i32 {
    a.wrapping_add((t * (b.wrapping_sub(a) as f32)).floor() as i32)
}
fn argb_lerp(a: i32, b: i32, t: f32) -> i32 {
    (0..4).fold(0u32, |color, i| {
        let av = (a as u32 >> (i * 8)) & 255;
        let bv = (b as u32 >> (i * 8)) & 255;
        let v = int_lerp(av as i32, bv as i32, t) as u32;
        color | (v << (i * 8))
    }) as i32
}
impl State {
    pub fn progress(&self, partial: f32) -> f32 {
        if self.duration <= 0 {
            1.
        } else {
            (((i64::from(self.age) - self.start_tick) as f32 + partial) / self.duration as f32)
                .clamp(0., 1.)
        }
    }
    pub fn transformation(&self, t: f32) -> Mat4 {
        self.from.interpolate(self.to, t).matrix()
    }
    pub fn background(&self, t: f32) -> u32 {
        argb_lerp(self.background_from, self.background_to, t) as u32
    }
    pub fn opacity(&self, t: f32) -> u8 {
        int_lerp(self.opacity_from, self.opacity_to, t) as u8
    }
    pub fn orientation(&self, entity_rotation: [f32; 2], camera_rotation: [f32; 2]) -> Quat {
        let [yaw, pitch] = entity_rotation;
        let [camera_yaw, camera_pitch] = camera_rotation;
        let (y, x) = match self.billboard {
            0 => (yaw, pitch),
            1 => (camera_yaw - 180., pitch),
            2 => (yaw, -camera_pitch),
            _ => (camera_yaw - 180., -camera_pitch),
        };
        Quat::from_euler(glam::EulerRot::YXZ, -y * 0.017453292, x * 0.017453292, 0.)
    }
}
