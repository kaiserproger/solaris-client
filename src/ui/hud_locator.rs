//! Source locator styles and geometry; packet state remains in HudPackets.
use super::{
    hud::{LocatorCamera, TrackedWaypoint, WaypointId, WaypointPosition},
    hud_math,
    state::{NodeAction, Paint, UiState},
};
use crate::protocol::{content::Content, parse_identifier, Result};
use serde_json::Value;
use std::sync::Arc;

pub(crate) enum WaypointStyle {
    Missing,
    Loaded {
        near: i32,
        far: i32,
        sprites: Vec<String>,
    },
}
impl WaypointStyle {
    pub(crate) fn load(content: &Content, id: &str) -> Result<Self> {
        let (namespace, path) = id.split_once(':').ok_or("waypoint_style_not_canonical")?;
        let stack =
            content.resource_stack(&format!("assets/{namespace}/waypoint_style/{path}.json"))?;
        let Some(bytes) = stack.last() else {
            return Ok(Self::Missing);
        };
        // SimpleJsonResourceReloadListener excludes a malformed resource; the manager then
        // returns its actual MISSING style. This is not a replacement default-dot style.
        match serde_json::from_slice::<Value>(bytes)
            .map_err(|e| e.to_string())
            .and_then(Self::parse)
        {
            Ok(style) => Ok(style),
            Err(error) => {
                eprintln!("waypoint style {id}: {error}");
                Ok(Self::Missing)
            }
        }
    }
    pub(crate) fn parse(value: Value) -> Result<Self> {
        let distance = |field: &str, default: i32| -> Result<i32> {
            match value.get(field) {
                None => Ok(default),
                Some(value) => {
                    let integer =
                        super::codec_int(value).map_err(|_| format!("waypoint_style_{field}"))?;
                    if (0..=60_000_000).contains(&integer) {
                        Ok(integer)
                    } else {
                        Err(format!("waypoint_style_{field}"))
                    }
                }
            }
        };
        let near = distance("near_distance", 128)?;
        let far = distance("far_distance", 332)?;
        if near <= 0 || near >= far {
            return Err("waypoint_style_distances".into());
        }
        let ids = value
            .get("sprites")
            .and_then(Value::as_array)
            .filter(|ids| !ids.is_empty())
            .ok_or("waypoint_style_sprites")?;
        let sprites = ids
            .iter()
            .map(|id| {
                let id = parse_identifier(id.as_str().ok_or("waypoint_sprite_identifier")?)?;
                let (namespace, path) = id.split_once(':').expect("canonical identifier");
                Ok(format!(
                    "{namespace}:gui/sprites/hud/locator_bar_dot/{path}"
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::Loaded { near, far, sprites })
    }
    pub(crate) fn sprite(&self, distance: f32) -> &str {
        let Self::Loaded { near, far, sprites } = self else {
            return "minecraft:missingno";
        };
        let index = if distance < *near as f32 || sprites.len() == 1 {
            0
        } else if distance >= *far as f32 {
            sprites.len() - 1
        } else if sprites.len() == 3 {
            1
        } else {
            1 + (((distance - *near as f32) / (*far - *near) as f32) * (sprites.len() - 2) as f32)
                .floor() as usize
        };
        &sprites[index]
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PitchDirection {
    None,
    Up,
    Down,
}
fn pitch(y: f64, behind: bool) -> PitchDirection {
    let adjusted = if behind { -y } else { y };
    if adjusted < -1. {
        PitchDirection::Down
    } else if adjusted > 1. {
        PitchDirection::Up
    } else if behind && y > 0. {
        PitchDirection::Up
    } else if behind && y < 0. {
        PitchDirection::Down
    } else {
        PitchDirection::None
    }
}
fn center(block: [i32; 3]) -> [f64; 3] {
    block.map(|n| f64::from(n) + 0.5)
}
fn chunk_center(chunk: [i32; 2], y: i32) -> [f64; 3] {
    center([
        chunk[0].wrapping_shl(4).wrapping_add(8),
        y,
        chunk[1].wrapping_shl(4).wrapping_add(8),
    ])
}
pub(crate) fn distance_squared(position: &WaypointPosition, receiver: [f64; 3]) -> f64 {
    let target = match position {
        WaypointPosition::Block(block) => center(*block),
        WaypointPosition::Chunk(chunk) => chunk_center(*chunk, receiver[1].floor() as i32),
        WaypointPosition::Empty | WaypointPosition::Azimuth(_) => return f64::INFINITY,
    };
    let [x, y, z] = std::array::from_fn(|i| receiver[i] - target[i]);
    x * x + y * y + z * z
}
fn degrees_difference(from: f32, to: f32) -> f32 {
    let mut value = (to - from) % 360.;
    if value >= 180. {
        value -= 360.;
    }
    if value < -180. {
        value += 360.;
    }
    value
}
pub(crate) fn geometry(
    id: &WaypointId,
    waypoint: &TrackedWaypoint,
    camera: &LocatorCamera,
) -> Option<(f64, PitchDirection)> {
    // Source excludes only the UUID receiver, never a same-spelled named identifier.
    if matches!(id, WaypointId::Uuid(_)) && id == &camera.receiver {
        return None;
    }
    let point = match waypoint.position {
        WaypointPosition::Empty => return None,
        WaypointPosition::Azimuth(angle) => {
            let yaw = degrees_difference(camera.yaw, angle * 57.295776) as f64;
            return (yaw > -60. && yaw <= 60.).then(|| {
                (
                    yaw,
                    camera
                        .horizon_screen_y
                        .map_or(PitchDirection::None, |y| pitch(y, false)),
                )
            });
        }
        WaypointPosition::Block(block) => camera
            .entity_eye_positions
            .get(id)
            .copied()
            .unwrap_or_else(|| center(block)),
        WaypointPosition::Chunk(chunk) => chunk_center(chunk, camera.position[1] as i32),
    };
    // Vec3.rotateClockwise90: (-z,y,x), with camera-target displacement.
    let x = camera.position[0] - point[0];
    let z = camera.position[2] - point[2];
    let target_yaw = hud_math::atan2(x, -z) as f32 * 57.295776;
    let yaw = f64::from(degrees_difference(camera.yaw, target_yaw));
    if !(yaw > -60. && yaw <= 60.) {
        return None;
    }
    let direction = match waypoint.position {
        WaypointPosition::Chunk(_) => camera
            .horizon_screen_y
            .map_or(PitchDirection::None, |y| pitch(y, false)),
        _ => camera
            .view_projection
            .map_or(PitchDirection::None, |matrix| {
                // GameRenderer subtracts camera in DOUBLE, then projects a FLOAT vector.
                let delta = glam::Vec3::new(
                    (point[0] - camera.position[0]) as f32,
                    (point[1] - camera.position[1]) as f32,
                    (point[2] - camera.position[2]) as f32,
                );
                let projected = matrix * delta.extend(1.);
                let normalized = projected.truncate() / projected.w;
                pitch(f64::from(normalized.y), normalized.z > 1.)
            }),
    };
    Some((yaw, direction))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextualBar {
    Empty,
    Jump,
    Experience,
    Locator,
}
pub(crate) fn select_bar(
    waypoints: bool,
    jumpable: bool,
    experience: bool,
    prioritize_jump: bool,
    prioritize_experience: bool,
) -> ContextualBar {
    if waypoints {
        if jumpable && prioritize_jump {
            ContextualBar::Jump
        } else if experience && prioritize_experience {
            ContextualBar::Experience
        } else {
            ContextualBar::Locator
        }
    } else if jumpable {
        ContextualBar::Jump
    } else if experience {
        ContextualBar::Experience
    } else {
        ContextualBar::Empty
    }
}
pub(crate) fn jump_width(scale: f32) -> i32 {
    (scale * 181.).floor() as i32 + i32::from(scale > 0.)
}
impl UiState {
    pub(crate) fn contextual_tree(
        &mut self,
        inventory: &crate::inventory::InventoryStore,
        content: &Content,
    ) -> Result<()> {
        let waypoints = !self.network_hud.waypoints.tracked.is_empty();
        let jumpable = self.hud.jump.is_some();
        let experience = matches!(inventory.game_mode, 0 | 2);
        let prioritize_jump = if waypoints && jumpable {
            self.hud.jump.is_some_and(|scale| scale > 0.)
                || self.hud.jump_cooldown.ok_or("jump_cooldown_unavailable")? > 0
        } else {
            false
        };
        let prioritize_experience = if waypoints && experience && !prioritize_jump {
            let start = self
                .hud
                .experience_display_start_tick
                .ok_or("experience_display_tick_unavailable")?;
            let tick = self.hud.player_tick.ok_or("player_tick_unavailable")?;
            start.wrapping_add(100) > tick
        } else {
            false
        };
        match select_bar(
            waypoints,
            jumpable,
            experience,
            prioritize_jump,
            prioritize_experience,
        ) {
            ContextualBar::Empty => Ok(()),
            ContextualBar::Locator => self.locator_tree(content),
            ContextualBar::Jump => {
                let width = jump_width(self.hud.jump.expect("jumpable selected"));
                let cooldown = self.hud.jump_cooldown.ok_or("jump_cooldown_unavailable")?;
                self.context_bar("jump", width, cooldown > 0)
            }
            ContextualBar::Experience => {
                if let Some(progress) = self.hud.experience {
                    self.context_bar("experience", (progress * 183.) as i32, false)?;
                }
                Ok(())
            }
        }
    }
    fn context_bar(&mut self, kind: &str, width: i32, cooldown: bool) -> Result<()> {
        // Vanilla logical pixels; UiState applies the selected GUI scale.
        let left = ((self.width as i32 - 182) / 2) as f32;
        let top = (self.height as i32 - 29) as f32;
        let scaled = width.clamp(0, 182) as f32;
        self.sprite(
            &format!("hud/{kind}/background"),
            &format!("gui/sprites/hud/{kind}_bar_background"),
            [left, top, 182., 5.],
        )?;
        if cooldown {
            return self.sprite(
                "hud/jump/cooldown",
                "gui/sprites/hud/jump_bar_cooldown",
                [left, top, 182., 5.],
            );
        }
        if scaled > 0. {
            self.add(
                format!("hud/{kind}/progress"),
                "progressbar",
                String::new(),
                [left, top, scaled, 5.],
                true,
                None,
                NodeAction::None,
                Paint::Image(
                    format!("gui/sprites/hud/{kind}_bar_progress"),
                    [0., 0., scaled / 182., 1.],
                ),
            )?;
        }
        Ok(())
    }
    pub(crate) fn locator_tree(&mut self, content: &Content) -> Result<()> {
        let Some(camera) = self.hud.locator.as_ref() else {
            return Ok(());
        };
        let top = self.height as i32 - 51;
        // Prepare paint entries before mutating the one ordinary retained semantic tree.
        let origin = ((self.width as i32 - 18) as f32 / 2.).ceil() as i32;
        let mut markers = Vec::with_capacity(self.network_hud.waypoints.tracked.len());
        for (id, waypoint) in &self.network_hud.waypoints.tracked {
            let distance = distance_squared(&waypoint.position, camera.receiver_position);
            let Some((yaw, direction)) = geometry(id, waypoint, camera) else {
                continue;
            };
            if !self.hud_waypoint_styles.contains_key(&waypoint.icon.style) {
                self.hud_waypoint_styles.insert(
                    waypoint.icon.style.clone(),
                    Arc::new(WaypointStyle::load(content, &waypoint.icon.style)?),
                );
            }
            let style = &self.hud_waypoint_styles[&waypoint.icon.style];
            let sprite = style.sprite((distance as f32).sqrt()).to_owned();
            let color = waypoint
                .icon
                .color
                .unwrap_or_else(|| hud_math::marker_color(id));
            let identifier = match id {
                WaypointId::Uuid(bytes) => {
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    let mut id = String::with_capacity(32);
                    for byte in bytes {
                        id.push(HEX[usize::from(byte >> 4)] as char);
                        id.push(HEX[usize::from(byte & 15)] as char);
                    }
                    id
                }
                WaypointId::Named(name) => format!("named/{name}"),
            };
            let x = origin + (yaw * 346. / 2. / 60.).floor() as i32;
            markers.push((identifier, sprite, color, x, direction, distance));
        }
        markers.sort_by(|a, b| b.5.total_cmp(&a.5));
        self.sprite(
            "hud/locator/background",
            "gui/sprites/hud/locator_bar_background",
            [
                ((self.width as i32 - 364) / 2) as f32,
                top as f32,
                364.,
                10.,
            ],
        )?;
        for (id, sprite, color, x, direction, _) in markers {
            self.add(
                format!("hud/locator/{id}/marker"),
                "presentation",
                String::new(),
                [x as f32, (top - 4) as f32, 18., 18.],
                true,
                None,
                NodeAction::None,
                Paint::TintedImage(
                    sprite,
                    [
                        ((color >> 16) & 255) as f32 / 255.,
                        ((color >> 8) & 255) as f32 / 255.,
                        (color & 255) as f32 / 255.,
                    ],
                ),
            )?;
            let (sprite, offset) = match direction {
                PitchDirection::None => continue,
                PitchDirection::Up => ("gui/sprites/hud/locator_bar_arrow_up", -12),
                PitchDirection::Down => ("gui/sprites/hud/locator_bar_arrow_down", 12),
            };
            self.sprite(
                &format!("hud/locator/{id}/arrow"),
                sprite,
                [(x + 2) as f32, (top + offset) as f32, 14., 10.],
            )?;
        }
        Ok(())
    }
}
