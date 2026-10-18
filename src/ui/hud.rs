//! Canonical scoreboard and waypoint state from the pinned 26.1.2 producers.
//! Wire layouts: Clientbound{SetObjective,SetDisplayObjective,SetScore,ResetScore,
//! SetPlayerTeam,TrackedWaypoint}Packet and their local javap codec captures.
use crate::protocol::{Reader, Result};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum NumberFormat {
    Blank,
    Styled(Arc<Value>),
    Fixed(Arc<Value>),
}
impl NumberFormat {
    pub fn component(&self, number: i32) -> Arc<Value> {
        match self {
            Self::Blank => Arc::new(json!({"text":""})),
            Self::Fixed(value) => Arc::clone(value),
            Self::Styled(style) => {
                let mut value = style.as_object().cloned().unwrap_or_default();
                value.insert("text".into(), Value::String(number.to_string()));
                Arc::new(Value::Object(value))
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ScoreRenderType {
    Integer,
    Hearts,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Objective {
    pub display: Arc<Value>,
    pub render_type: ScoreRenderType,
    pub number_format: Option<NumberFormat>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScoreEntry {
    pub value: i32,
    pub display: Option<Arc<Value>>,
    pub number_format: Option<NumberFormat>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TeamVisibility {
    Always,
    Never,
    HideForOtherTeams,
    HideForOwnTeam,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TeamCollision {
    Always,
    Never,
    PushOtherTeams,
    PushOwnTeam,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TeamParameters {
    pub display: Arc<Value>,
    pub options: u8,
    pub visibility: TeamVisibility,
    pub collision: TeamCollision,
    /// ChatFormatting enum ordinal, including the six non-color formatting values.
    pub formatting: u8,
    pub prefix: Arc<Value>,
    pub suffix: Arc<Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScoreTeam {
    pub parameters: TeamParameters,
    pub members: BTreeSet<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ScoreboardState {
    pub objectives: BTreeMap<String, Objective>,
    /// Objective name -> score holder -> score. Holders need not be players.
    pub scores: BTreeMap<String, BTreeMap<String, ScoreEntry>>,
    pub display_slots: [Option<String>; 19],
    pub teams: BTreeMap<String, ScoreTeam>,
    pub membership: BTreeMap<String, String>,
}
impl ScoreboardState {
    pub fn team(&self, holder: &str) -> Option<&ScoreTeam> {
        self.teams.get(self.membership.get(holder)?)
    }
    pub fn sidebar_objective(&self, local_holder: Option<&str>) -> Option<(&str, &Objective)> {
        let team_slot = local_holder
            .and_then(|holder| self.team(holder))
            .map(|team| team.parameters.formatting)
            .filter(|color| *color < 16)
            .and_then(|color| self.display_slots[usize::from(color) + 3].as_deref());
        let name = team_slot.or(self.display_slots[1].as_deref())?;
        Some((name, self.objectives.get(name)?))
    }
    pub fn formatted_name(&self, holder: &str, display: Option<&Arc<Value>>) -> Arc<Value> {
        let name = display
            .cloned()
            .unwrap_or_else(|| Arc::new(json!({"text":holder})));
        let Some(team) = self.team(holder) else {
            return name;
        };
        let p = &team.parameters;
        let mut value =
            json!({"text":"", "extra":[p.prefix.as_ref(), name.as_ref(), p.suffix.as_ref()]});
        if let Some(color) = formatting_color(p.formatting) {
            value["color"] = Value::String(color.into());
        }
        Arc::new(value)
    }
    pub fn score_component(
        &self,
        objective: &Objective,
        score: &ScoreEntry,
        sidebar: bool,
    ) -> Arc<Value> {
        match score
            .number_format
            .as_ref()
            .or(objective.number_format.as_ref())
        {
            Some(format) => format.component(score.value),
            None => Arc::new(
                json!({"text":score.value.to_string(),"color":if sidebar {"red"} else {"yellow"}}),
            ),
        }
    }
    fn remove_team(&mut self, name: &str) {
        if let Some(team) = self.teams.remove(name) {
            for member in &team.members {
                self.membership.remove(member);
            }
        }
    }
}
pub(crate) fn formatting_color(ordinal: u8) -> Option<&'static str> {
    [
        "black",
        "dark_blue",
        "dark_green",
        "dark_aqua",
        "dark_red",
        "dark_purple",
        "gold",
        "gray",
        "dark_gray",
        "blue",
        "green",
        "aqua",
        "red",
        "light_purple",
        "yellow",
        "white",
    ]
    .get(usize::from(ordinal))
    .copied()
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum WaypointId {
    Uuid([u8; 16]),
    Named(String),
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WaypointIcon {
    pub style: String,
    pub color: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum WaypointPosition {
    Empty,
    Block([i32; 3]),
    Chunk([i32; 2]),
    Azimuth(f32),
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrackedWaypoint {
    pub icon: WaypointIcon,
    pub position: WaypointPosition,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WaypointState {
    #[serde(serialize_with = "serialize_waypoints")]
    pub tracked: BTreeMap<WaypointId, TrackedWaypoint>,
}
fn serialize_waypoints<S: serde::Serializer>(
    tracked: &BTreeMap<WaypointId, TrackedWaypoint>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.collect_seq(tracked.iter())
}
/// Actual camera/receiver inputs. Absence is represented by HudState.locator=None.
#[derive(Clone, Debug, PartialEq)]
pub struct LocatorCamera {
    pub receiver: WaypointId,
    pub position: [f64; 3],
    pub receiver_position: [f64; 3],
    pub yaw: f32,
    /// Source FLOAT view-rotation-projection; translation is subtracted before conversion to f32.
    pub view_projection: Option<glam::Mat4>,
    pub horizon_screen_y: Option<f64>,
    /// Only entities observed within Manhattan distance <=3 of a block waypoint.
    pub entity_eye_positions: BTreeMap<WaypointId, [f64; 3]>,
}
#[derive(Default)]
pub(crate) struct HudPackets {
    pub scoreboard: ScoreboardState,
    pub waypoints: WaypointState,
}

enum Packet {
    Objective(String, u8, Option<Objective>),
    Display(usize, String),
    Score(String, String, ScoreEntry),
    Reset(String, Option<String>),
    Team(String, u8, Option<TeamParameters>, Vec<String>),
    Waypoint(i32, WaypointId, TrackedWaypoint),
}
fn optional_component(r: &mut Reader<'_>) -> Result<Option<Arc<Value>>> {
    if r.bool()? {
        Ok(Some(Arc::new(r.nbt()?)))
    } else {
        Ok(None)
    }
}
fn optional_format<'a>(
    r: &mut Reader<'_>,
    resolve: &impl Fn(i32) -> Result<&'a str>,
) -> Result<Option<NumberFormat>> {
    if !r.bool()? {
        return Ok(None);
    }
    Ok(Some(match resolve(r.var()?)? {
        "minecraft:blank" => NumberFormat::Blank,
        "minecraft:styled" => {
            let style = r.nbt()?;
            if !style.is_object() {
                return Err("score_number_style_not_compound".into());
            }
            NumberFormat::Styled(Arc::new(style))
        }
        "minecraft:fixed" => NumberFormat::Fixed(Arc::new(r.nbt()?)),
        name => return Err(format!("unsupported_number_format:{name}")),
    }))
}
fn decode<'a>(
    name: &str,
    bytes: &[u8],
    resolve: &impl Fn(i32) -> Result<&'a str>,
) -> Result<Option<Packet>> {
    let mut r = Reader::new(bytes);
    let packet = match name {
        "SET_OBJECTIVE" => {
            let name = r.string(32767)?;
            let method = r.u8()?;
            let objective = if matches!(method, 0 | 2) {
                let display = Arc::new(r.nbt()?);
                let render_type = match r.var()? {
                    0 => ScoreRenderType::Integer,
                    1 => ScoreRenderType::Hearts,
                    _ => return Err("invalid_score_render_type".into()),
                };
                Some(Objective {
                    display,
                    render_type,
                    number_format: optional_format(&mut r, resolve)?,
                })
            } else {
                None
            };
            Packet::Objective(name, method, objective)
        }
        "SET_DISPLAY_OBJECTIVE" => {
            let id = r.var()?;
            let slot = if (0..19).contains(&id) {
                id as usize
            } else {
                0
            };
            Packet::Display(slot, r.string(32767)?)
        }
        "SET_SCORE" => {
            let holder = r.string(32767)?;
            let objective = r.string(32767)?;
            let score = ScoreEntry {
                value: r.var()?,
                display: optional_component(&mut r)?,
                number_format: optional_format(&mut r, resolve)?,
            };
            Packet::Score(holder, objective, score)
        }
        "RESET_SCORE" => {
            let holder = r.string(32767)?;
            let objective = if r.bool()? {
                Some(r.string(32767)?)
            } else {
                None
            };
            Packet::Reset(holder, objective)
        }
        "SET_PLAYER_TEAM" => {
            let name = r.string(32767)?;
            let method = r.u8()?;
            let parameters = if matches!(method, 0 | 2) {
                let display = Arc::new(r.nbt()?);
                let options = r.u8()?;
                let visibility = match r.var()? {
                    1 => TeamVisibility::Never,
                    2 => TeamVisibility::HideForOtherTeams,
                    3 => TeamVisibility::HideForOwnTeam,
                    _ => TeamVisibility::Always,
                };
                let collision = match r.var()? {
                    1 => TeamCollision::Never,
                    2 => TeamCollision::PushOtherTeams,
                    3 => TeamCollision::PushOwnTeam,
                    _ => TeamCollision::Always,
                };
                let formatting = r.var()?;
                if !(0..22).contains(&formatting) {
                    return Err("invalid_team_formatting".into());
                }
                Some(TeamParameters {
                    display,
                    options,
                    visibility,
                    collision,
                    formatting: formatting as u8,
                    prefix: Arc::new(r.nbt()?),
                    suffix: Arc::new(r.nbt()?),
                })
            } else {
                None
            };
            let members = if matches!(method, 0 | 3 | 4) {
                let count = r.count(65536)?;
                (0..count)
                    .map(|_| r.string(32767))
                    .collect::<Result<Vec<_>>>()?
            } else {
                Vec::new()
            };
            Packet::Team(name, method, parameters, members)
        }
        "WAYPOINT" => {
            let operation = r.var()?.rem_euclid(3);
            let id = if r.bool()? {
                WaypointId::Uuid(r.take(16)?.try_into().unwrap())
            } else {
                WaypointId::Named(r.string(32767)?)
            };
            let style = crate::protocol::parse_identifier(&r.string(32767)?)?;
            let color = if r.bool()? {
                let rgb = r.take(3)?;
                Some(
                    0xff000000
                        | u32::from(rgb[0]) << 16
                        | u32::from(rgb[1]) << 8
                        | u32::from(rgb[2]),
                )
            } else {
                None
            };
            let position = match r.var()? {
                0 => WaypointPosition::Empty,
                1 => WaypointPosition::Block([r.var()?, r.var()?, r.var()?]),
                2 => WaypointPosition::Chunk([r.var()?, r.var()?]),
                3 => WaypointPosition::Azimuth(r.f32()?),
                _ => return Err("invalid_waypoint_type".into()),
            };
            Packet::Waypoint(
                operation,
                id,
                TrackedWaypoint {
                    icon: WaypointIcon { style, color },
                    position,
                },
            )
        }
        _ => return Ok(None),
    };
    r.done()?;
    Ok(Some(packet))
}
impl HudPackets {
    pub fn apply<'a>(
        &mut self,
        name: &str,
        bytes: &[u8],
        resolve: impl Fn(i32) -> Result<&'a str>,
    ) -> Result<bool> {
        let Some(packet) = decode(name, bytes, &resolve)? else {
            return Ok(false);
        };
        let board = &mut self.scoreboard;
        match packet {
            Packet::Objective(name, method, objective) => match method {
                0 => {
                    if board.objectives.contains_key(&name) {
                        return Err("score_objective_already_exists".into());
                    }
                    board.objectives.insert(name, objective.unwrap());
                }
                1 => {
                    board.objectives.remove(&name);
                    board.scores.remove(&name);
                    for slot in &mut board.display_slots {
                        if slot.as_ref() == Some(&name) {
                            *slot = None;
                        }
                    }
                }
                2 => {
                    if let Some(old) = board.objectives.get_mut(&name) {
                        *old = objective.unwrap();
                    }
                }
                _ => {}
            },
            Packet::Display(slot, name) => {
                board.display_slots[slot] = board.objectives.contains_key(&name).then_some(name)
            }
            Packet::Score(holder, objective, score) => {
                if board.objectives.contains_key(&objective) {
                    board
                        .scores
                        .entry(objective)
                        .or_default()
                        .insert(holder, score);
                }
            }
            Packet::Reset(holder, objective) => {
                if let Some(objective) = objective {
                    if let Some(scores) = board.scores.get_mut(&objective) {
                        scores.remove(&holder);
                    }
                } else {
                    for scores in board.scores.values_mut() {
                        scores.remove(&holder);
                    }
                }
            }
            Packet::Team(name, method, mut parameters, members) => {
                if method == 1 {
                    board.remove_team(&name);
                } else if matches!(method, 0 | 2 | 3 | 4) {
                    if method == 0 && !board.teams.contains_key(&name) {
                        board.teams.insert(
                            name.clone(),
                            ScoreTeam {
                                parameters: parameters.take().unwrap(),
                                members: BTreeSet::new(),
                            },
                        );
                    }
                    if board.teams.contains_key(&name) {
                        if let Some(parameters) = parameters {
                            board.teams.get_mut(&name).unwrap().parameters = parameters;
                        }
                        if matches!(method, 0 | 3) {
                            for member in members {
                                if let Some(old) =
                                    board.membership.insert(member.clone(), name.clone())
                                {
                                    if let Some(team) = board.teams.get_mut(&old) {
                                        team.members.remove(&member);
                                    }
                                }
                                board.teams.get_mut(&name).unwrap().members.insert(member);
                            }
                        } else if method == 4 {
                            if members
                                .iter()
                                .any(|member| board.membership.get(member) != Some(&name))
                            {
                                return Err("score_member_not_in_team".into());
                            }
                            for member in members {
                                board.membership.remove(&member);
                                board.teams.get_mut(&name).unwrap().members.remove(&member);
                            }
                        }
                    }
                }
            }
            Packet::Waypoint(operation, id, waypoint) => match operation {
                0 => {
                    self.waypoints.tracked.insert(id, waypoint);
                }
                1 => {
                    self.waypoints.tracked.remove(&id);
                }
                2 => {
                    let old = self
                        .waypoints
                        .tracked
                        .get_mut(&id)
                        .ok_or("waypoint_update_not_tracked")?;
                    // Source update methods retain the original icon and only update same-family contents.
                    if std::mem::discriminant(&old.position)
                        == std::mem::discriminant(&waypoint.position)
                    {
                        old.position = waypoint.position;
                    }
                }
                _ => unreachable!(),
            },
        }
        Ok(true)
    }
}
