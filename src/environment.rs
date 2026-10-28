//! Target 26.1.2 received environment state and camera attribute evaluation.
//! Provenance: local verified client archive, EnvironmentAttributeSystem/Probe,
//! GaussianSampler, ClientClockManager, ClockNetworkState, WeatherAttributes,
//! FogRenderer and its six environment classes, SkyRenderer and EndFlashState.
//! Dimension/biome/timeline values are received registry facts, never dimension
//! names mapped to fabricated legacy day/color tables. Parent owns epochs.
use crate::protocol::packet_ids::play::clientbound as cb;
mod attributes;
pub(crate) mod biome;
mod fading;
mod lightmap;
mod point;
use crate::api::{LightmapTexture, SkyKind, SkyTexture, WorldEnvironment};
use crate::protocol::{Reader, Result, content::Content, world::World};
use attributes::{
    ALL, Attribute as A, AttributeMap, COUNT, Datum, Track, apply_map, color_lerp, defaults, lerp,
    multiply, number, parse_map, rgb, rgba, scale_rgb,
};
pub use fading::{DarknessBlend, DarknessSnapshot};
pub use lightmap::LightmapInputs;
use lightmap::{LightmapCache, Random, cos, sin};
pub(crate) use point::PointState;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMedium {
    #[default]
    Atmospheric,
    Water,
    Lava,
    PowderSnow,
}
/// Live facts supplied by the player/session owners, not a second effect store.
#[derive(Clone, Copy, Debug)]
pub struct PlayerEnvironmentFacts {
    pub medium: CameraMedium,
    pub spectator: bool,
    pub fire_resistance: bool,
    /// -1 denotes the target infinite duration representation.
    pub night_vision_duration: Option<i32>,
    pub blindness_duration: Option<i32>,
    pub darkness_present: bool,
    pub darkness_blend: f32,
    pub water_vision: f32,
    pub conduit_power: bool,
    pub entity_tick: u64,
    pub gamma: f32,
    pub darkness_effect_scale: f32,
    pub boss_darkening: f32,
    pub boss_fog: bool,
    pub hide_lightning_flashes: bool,
    /// Login/respawn isFlat fact controls horizon and void-fog scale.
    pub flat_world: bool,
}
impl Default for PlayerEnvironmentFacts {
    fn default() -> Self {
        Self {
            medium: CameraMedium::Atmospheric,
            spectator: false,
            fire_resistance: false,
            night_vision_duration: None,
            blindness_duration: None,
            darkness_present: false,
            darkness_blend: 0.,
            water_vision: 0.,
            conduit_power: false,
            entity_tick: 0,
            gamma: 0.5,
            darkness_effect_scale: 1.,
            boss_darkening: 0.,
            boss_fog: false,
            hide_lightning_flashes: false,
            flat_world: false,
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Clock {
    ticks: i64,
    partial: f32,
    rate: f32,
}
impl Clock {
    fn advance(&mut self, delta: i64) {
        let time = self.partial as f64 + delta as f64 * self.rate as f64;
        let whole = time.floor() as i64;
        self.partial = (time - whole as f64) as f32;
        self.ticks = self.ticks.wrapping_add(whole);
    }
}
#[derive(Clone, Debug)]
struct Timeline {
    clock: String,
    period: Option<i64>,
    tracks: Vec<Track>,
}
impl Timeline {
    fn parse(value: &Value) -> Result<Self> {
        let clock = value["clock"]
            .as_str()
            .ok_or("timeline clock missing")?
            .to_owned();
        let period = value
            .get("period_ticks")
            .map(|v| {
                v.as_i64()
                    .filter(|v| *v > 0)
                    .ok_or("timeline period must be positive")
            })
            .transpose()?;
        let mut tracks = Vec::new();
        if let Some(values) = value.get("tracks") {
            for (name, v) in values.as_object().ok_or("timeline tracks must be map")? {
                if let Some(a) = A::from_name(name) {
                    tracks.push(Track::parse(a, v, period)?);
                }
            }
        }
        Ok(Self {
            clock,
            period,
            tracks,
        })
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct EndFlash {
    seed: i64,
    offset: i32,
    duration: i32,
    old: f32,
    intensity: f32,
    x: f32,
    y: f32,
}
impl EndFlash {
    fn tick(&mut self, time: i64) {
        let seed = time / 600;
        if self.seed != seed {
            let mut random = Random::new(seed as u64);
            random.float();
            self.offset = random.int(201);
            self.duration = 100 + random.int(281);
            self.x = -60. + random.float() * 70.;
            self.y = -180. + random.float() * 360.;
            self.seed = seed;
        }
        self.old = self.intensity;
        let t = time % 600;
        self.intensity = if self.duration <= 0
            || t < self.offset as i64
            || t > (self.offset + self.duration) as i64
        {
            0.
        } else {
            sin(
                ((t - self.offset as i64) as f32 * std::f32::consts::PI / self.duration as f32)
                    as f64,
            )
        };
    }
}
pub struct Environment {
    dimension: Option<Dimension>,
    timelines: Vec<(String, Timeline)>,
    timeline_tags: BTreeMap<String, Vec<i32>>,
    clock_names: Vec<String>,
    clocks: BTreeMap<String, Clock>,
    biomes: Arc<Vec<AttributeMap>>,
    point_rules: Option<Arc<point::PointRules>>,
    precipitation: Vec<bool>,
    game_time: i64,
    last_clock_game_time: i64,
    client_tick: u64,
    partial: f32,
    pub tick_rate: f32,
    pub frozen: bool,
    steps: u32,
    rain: f32,
    thunder: f32,
    rain_fog: f32,
    last_fog_time: Option<f64>,
    probe_tick: Option<u64>,
    probe_old: [Datum; COUNT],
    probe_new: [Datum; COUNT],
    flash: EndFlash,
    flicker: f32,
    random: Random,
    lightmap: LightmapCache,
}
struct Dimension {
    attributes: AttributeMap,
    timeline_ids: Vec<usize>,
    default_clock: Option<String>,
    sky_kind: SkyKind,
    light_directions: [[f32; 3]; 2],
    weather: bool,
    min_y: i32,
    height: i32,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            dimension: None,
            timelines: Vec::new(),
            timeline_tags: BTreeMap::new(),
            clock_names: Vec::new(),
            clocks: BTreeMap::new(),
            biomes: Arc::new(Vec::new()),
            point_rules: None,
            precipitation: Vec::new(),
            game_time: 0,
            last_clock_game_time: 0,
            client_tick: 0,
            partial: 0.,
            tick_rate: 20.,
            frozen: false,
            steps: 0,
            rain: 0.,
            thunder: 0.,
            rain_fog: 0.,
            last_fog_time: None,
            probe_tick: None,
            probe_old: defaults(),
            probe_new: defaults(),
            flash: EndFlash::default(),
            flicker: 0.,
            random: Random::new(0),
            lightmap: LightmapCache::default(),
        }
    }
}
impl Environment {
    pub fn new() -> Self {
        Self::default()
    }
    /// Configuration values retain only environment-bearing registry tables.
    pub fn configure_registry(&mut self, name: &str, entries: &[(String, Value)]) -> Result<()> {
        match name {
            "minecraft:timeline" => {
                let mut parsed = Vec::with_capacity(entries.len());
                for (name, v) in entries {
                    parsed.push((name.clone(), Timeline::parse(v)?));
                }
                self.timelines = parsed;
            }
            "minecraft:world_clock" => {
                self.clock_names = entries.iter().map(|v| v.0.clone()).collect();
                for name in &self.clock_names {
                    self.clocks.entry(name.clone()).or_insert(Clock {
                        ticks: 0,
                        partial: 0.,
                        rate: 1.,
                    });
                }
            }
            "minecraft:worldgen/biome" => {
                let mut maps = Vec::with_capacity(entries.len());
                let mut precipitation = Vec::with_capacity(entries.len());
                for (_, v) in entries {
                    maps.push(parse_map(&v["attributes"])?);
                    precipitation.push(match &v["has_precipitation"] {
                        Value::Bool(v) => *v,
                        Value::Number(v) => v.as_i64() != Some(0),
                        _ => return Err("biome precipitation missing".into()),
                    });
                }
                self.biomes = Arc::new(maps);
                self.precipitation = precipitation;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn configure_tags(
        &mut self,
        registry: &str,
        tags: &BTreeMap<String, Vec<i32>>,
    ) -> Result<()> {
        if registry == "minecraft:timeline" {
            self.timeline_tags = tags.clone();
        }
        Ok(())
    }
    pub fn activate_dimension(&mut self, name: &str, data: &Value, tick: u64) -> Result<()> {
        let min_y = data["min_y"].as_i64().ok_or("dimension min_y missing")? as i32;
        let height = data["height"]
            .as_i64()
            .filter(|v| *v > 0 && *v <= 4096)
            .ok_or("dimension height invalid")? as i32;
        let sky_kind = match data
            .get("skybox")
            .and_then(Value::as_str)
            .unwrap_or("overworld")
        {
            "none" => SkyKind::None,
            "overworld" => SkyKind::Overworld,
            "end" => SkyKind::End,
            v => return Err(format!("unknown dimension skybox {v}")),
        };
        // Pinned 26.1.2 DimensionType defaults cardinal_light to "default";
        // Lighting.updateLevel selects these normalized source vectors.
        let second_y = match data.get("cardinal_light") {
            None => 1.,
            Some(Value::String(kind)) if kind == "default" => 1.,
            Some(Value::String(kind)) if kind == "nether" => -1.,
            _ => return Err("unknown dimension cardinal_light".into()),
        };
        let light_directions = [
            glam::Vec3::new(0.2, 1., -0.7).normalize().to_array(),
            glam::Vec3::new(-0.2, second_y, 0.7).normalize().to_array(),
        ];
        let mut timeline_ids = Vec::new();
        match data.get("timelines") {
            Some(Value::String(tag)) if tag.starts_with('#') => {
                for id in self
                    .timeline_tags
                    .get(&tag[1..])
                    .ok_or_else(|| format!("unknown dimension timeline tag {tag}"))?
                {
                    if *id < 0 || *id as usize >= self.timelines.len() {
                        return Err("dimension timeline tag holder out of range".into());
                    }
                    timeline_ids.push(*id as usize);
                }
            }
            Some(Value::Array(ids)) => {
                for id in ids {
                    let i = if let Some(n) = id.as_i64() {
                        usize::try_from(n).map_err(|_| "negative timeline holder")?
                    } else {
                        let name = id.as_str().ok_or("timeline holder requires name/id")?;
                        self.timelines
                            .iter()
                            .position(|v| v.0 == name)
                            .ok_or_else(|| format!("unknown timeline {name}"))?
                    };
                    if i >= self.timelines.len() {
                        return Err("timeline holder out of range".into());
                    }
                    timeline_ids.push(i);
                }
            }
            None => {}
            _ => return Err("dimension timelines require tag or holder list".into()),
        }
        let has_skylight = nbt_bool(data, "has_skylight")?;
        let has_ceiling = nbt_bool(data, "has_ceiling")?;
        let default_clock = data
            .get("default_clock")
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or("dimension clock requires name")
            })
            .transpose()?;
        if let Some(clock) = &default_clock {
            if !self.clocks.contains_key(clock) {
                return Err(format!("dimension references missing clock {clock}"));
            }
        }
        for id in &timeline_ids {
            let clock = &self.timelines[*id].1.clock;
            if !self.clocks.contains_key(clock) {
                return Err(format!("timeline references missing clock {clock}"));
            }
        }
        self.dimension = Some(Dimension {
            attributes: parse_map(&data["attributes"])?,
            timeline_ids,
            default_clock,
            sky_kind,
            light_directions,
            weather: has_skylight && !has_ceiling && name != "minecraft:the_end",
            min_y,
            height,
        });
        self.point_rules = Some(Arc::new(point::PointRules::new(self, has_skylight)?));
        self.client_tick = tick;
        self.partial = 0.;
        self.probe_tick = None;
        self.flash = EndFlash::default();
        self.rain_fog = 0.;
        self.last_fog_time = None;
        self.rain = 0.;
        self.thunder = 0.;
        Ok(())
    }
    /// Advance is called on the client's game tick owner, never renderer time.
    /// Frozen ticks still advance presentation flicker, as does target extractor;
    /// game time/clocks advance only on normal or explicitly stepped ticks.
    pub fn advance(&mut self, tick: u64, partial: f32) {
        while self.client_tick < tick {
            self.client_tick += 1;
            if !self.frozen || self.steps > 0 {
                self.steps = self.steps.saturating_sub(1);
                self.game_time = self.game_time.wrapping_add(1);
                self.advance_clocks(self.game_time);
            }
            self.flicker = (self.flicker
                + (self.random.float() - self.random.float())
                    * self.random.float()
                    * self.random.float()
                    * 0.1)
                * 0.9;
            let time = self.default_clock_time();
            self.flash.tick(time);
        }
        self.partial = if self.frozen {
            1.
        } else {
            partial.clamp(0., 1.)
        };
    }
    pub fn tick(&mut self, tick: u64) {
        self.advance(tick, 0.);
    }
    fn advance_clocks(&mut self, time: i64) {
        let delta = time.wrapping_sub(self.last_clock_game_time);
        for c in self.clocks.values_mut() {
            c.advance(delta);
        }
        self.last_clock_game_time = time;
    }
    pub fn game_time(&self) -> i64 {
        self.game_time
    }
    /// Publish on activation, owned environment packets and game ticks before
    /// gameplay queries. This shares immutable registry/track facts and updates
    /// only bounded clock scalars; neither rendering nor World advances clocks.
    pub fn publish_world(&self, world: &mut World) -> Result<()> {
        world.environment.publish(self)
    }
    pub fn default_clock_time(&self) -> i64 {
        self.dimension
            .as_ref()
            .and_then(|d| d.default_clock.as_ref())
            .and_then(|n| self.clocks.get(n))
            .map_or(0, |c| c.ticks)
    }
    /// Only consumes recognized owned packet variants. Other game events leave
    /// Reader unchanged so the owning player/UI domain can apply them.
    pub fn apply(
        &mut self,
        id: i32,
        r: &mut Reader<'_>,
        content: &Content,
        tick: u64,
    ) -> Result<bool> {
        if !matches!(id, cb::GAME_EVENT | cb::SET_TIME | cb::TICKING_STATE | cb::TICKING_STEP) {
            return Ok(false);
        }
        if id == cb::GAME_EVENT && !matches!(r.bytes.get(r.offset), Some(1 | 2 | 7 | 8)) {
            return Ok(false);
        }
        self.advance(tick, self.partial);
        match id {
            cb::GAME_EVENT => {
                let event = r.u8()?;
                let v = r.f32()?.clamp(0., 1.);
                match event {
                    1 => self.rain = 0.,
                    2 => self.rain = 1.,
                    7 => self.rain = v,
                    8 => self.thunder = v,
                    _ => unreachable!(),
                }
            }
            cb::SET_TIME => {
                let time = r.i64()?;
                let count = r.count(1024)?;
                let mut updates = Vec::with_capacity(count);
                for _ in 0..count {
                    let id = r.var()?;
                    let name = content.registry_name("minecraft:world_clock", id)?;
                    if !self.clocks.contains_key(name) {
                        return Err(format!("clock update lacks configured holder {name}"));
                    }
                    let ticks = r.varlong()? as i64;
                    let partial = r.f32()?;
                    let rate = r.f32()?;
                    if !(0. ..1.).contains(&partial) {
                        return Err("clock partial out of range".into());
                    }
                    if updates.iter().any(|(n, _)| n == &name) {
                        return Err("duplicate clock update".into());
                    }
                    updates.push((
                        name,
                        Clock {
                            ticks,
                            partial,
                            rate,
                        },
                    ));
                }
                self.advance_clocks(time);
                self.game_time = time;
                for (name, c) in updates {
                    *self
                        .clocks
                        .get_mut(name)
                        .expect("configured clock checked before commit") = c;
                }
            }
            cb::TICKING_STATE => {
                let rate = r.f32()?;
                let frozen = r.bool()?;
                if !rate.is_finite() || rate <= 0. {
                    return Err("nonpositive server tick rate".into());
                }
                self.tick_rate = rate;
                self.frozen = frozen;
            }
            cb::TICKING_STEP => {
                self.steps = r.count(i32::MAX as usize)? as u32;
            }
            _ => unreachable!(),
        }
        Ok(true)
    }
    pub fn lightmap_texture(&self) -> Option<LightmapTexture> {
        self.lightmap.texture()
    }
    pub fn lightmap_inputs(&self) -> Option<LightmapInputs> {
        self.lightmap.inputs()
    }
    fn camera_attributes(&self, world: &World, camera: [f64; 3]) -> Result<[Datum; COUNT]> {
        let dimension = self
            .dimension
            .as_ref()
            .ok_or("environment dimension not activated")?;
        let base = apply_map(&dimension.attributes, defaults());
        let position = camera.map(|v| v * 0.25 - 0.5);
        let floor = position.map(|v| v.floor() as i32);
        let frac = std::array::from_fn::<_, 3, _>(|i| position[i] - floor[i] as f64);
        let kernel = [0., 1., 4., 6., 4., 1., 0.];
        // Coalesce Gaussian samples by actual biome ID. Fixed bounded storage,
        // preserving target z/x/y traversal and spatial lerp quantization.
        let mut sources = [(0u32, 0f64); 216];
        let mut length = 0;
        for z in 0..6 {
            for x in 0..6 {
                for y in 0..6 {
                    let weight = (kernel[x + 1] + frac[0] * (kernel[x] - kernel[x + 1]))
                        * (kernel[y + 1] + frac[1] * (kernel[y] - kernel[y + 1]))
                        * (kernel[z + 1] + frac[2] * (kernel[z] - kernel[z + 1]));
                    if weight == 0. {
                        continue;
                    }
                    let p = [
                        (floor[0] - 2 + x as i32) * 4,
                        ((floor[1] - 2 + y as i32) * 4)
                            .clamp(dimension.min_y, dimension.min_y + dimension.height - 1),
                        (floor[2] - 2 + z as i32) * 4,
                    ];
                    // A sample whose chunk has not arrived (long teleport, view
                    // edge) is absent and only drops out of the blend; with none
                    // loaded the dimension's biome-less attributes apply. It never
                    // ends the session.
                    let Some(id) = world.biome(p) else { continue };
                    if let Some(i) = sources[..length].iter().position(|v| v.0 == id) {
                        sources[i].1 += weight;
                    } else {
                        sources[length] = (id, weight);
                        length += 1;
                    }
                }
            }
        }
        let mut values = base;
        let mut total = 0.;
        for &(id, weight) in &sources[..length] {
            let map = self
                .biomes
                .get(id as usize)
                .ok_or("environment biome registry holder missing")?;
            let modified = apply_map(map, base);
            total += weight;
            for i in 0..COUNT {
                if ALL[i].positional() && !ALL[i].boolean() {
                    values[i] = if total == weight {
                        modified[i]
                    } else {
                        ALL[i].interpolate(values[i], modified[i], (weight / total) as f32, false)
                    };
                }
            }
        }
        // Moon phase is non-positional. Timelines are layered after biomes.
        for id in &dimension.timeline_ids {
            let timeline = &self.timelines[*id].1;
            let time = self.clocks.get(&timeline.clock).map_or(0, |c| c.ticks);
            for track in &timeline.tracks {
                let i = track.attribute as usize;
                values[i] = track.apply(values[i], time, timeline.period)?;
            }
        }
        if dimension.weather {
            apply_weather(&mut values, self.rain, self.thunder * self.rain);
        }
        Ok(values)
    }
    /// Rain-fog target at the camera block. A camera chunk (or its sky light)
    /// that has not arrived yet contributes no rain fog rather than failing the
    /// session; the fog eases toward the real target once the data is received.
    fn rain_fog_target(&self, world: &World, p: [i32; 3], weather: bool, rain: f32) -> Result<f32> {
        let Some(biome) = world.biome(p) else {
            return Ok(0.);
        };
        let sky = if weather && rain > 0. {
            world.light_levels(p)[0].map_or(0., f32::from)
        } else {
            0.
        };
        let precip = *self
            .precipitation
            .get(biome as usize)
            .ok_or("camera biome precipitation unknown")?;
        Ok(rain * ((sky - 8.) / 7.).clamp(0., 1.) * if precip { 1. } else { 0.5 })
    }
    pub fn evaluate(
        &mut self,
        world: &World,
        content: &Content,
        camera: [f64; 3],
        yaw: f32,
        pitch: f32,
        view_distance: u32,
        facts: &PlayerEnvironmentFacts,
    ) -> Result<WorldEnvironment> {
        if !camera.iter().all(|v| v.is_finite()) || !yaw.is_finite() || !pitch.is_finite() {
            return Err("environment camera nonfinite".into());
        }
        if self.probe_tick != Some(self.client_tick) {
            let values = self.camera_attributes(world, camera)?;
            self.probe_old = if self.probe_tick.is_some() {
                self.probe_new
            } else {
                values
            };
            self.probe_tick = Some(self.client_tick);
            self.probe_new = values;
        }
        let v = std::array::from_fn::<_, COUNT, _>(|i| {
            ALL[i].interpolate(self.probe_old[i], self.probe_new[i], self.partial, true)
        });
        let color = |a: A| v[a as usize].color();
        let float = |a: A| v[a as usize].float();
        let dimension = self
            .dimension
            .as_ref()
            .ok_or("environment dimension missing")?;
        let distance = view_distance as f32 * 16.;
        let weather = dimension.weather;
        let rain = if weather { self.rain } else { 0. };
        let thunder = if weather {
            self.thunder * self.rain
        } else {
            0.
        };
        let mut result = WorldEnvironment {
            display_encoded: true,
            sky_color: rgb(color(A::SkyColor)),
            fog_color: rgb(color(A::FogColor)),
            fog_start: float(A::FogStart),
            fog_end: float(A::FogEnd),
            sky_fog_end: distance.min(float(A::SkyFogEnd)),
            render_fog_start: distance - (distance / 10.).clamp(4., 64.),
            render_fog_end: distance,
            fog_alpha: 1.,
            time_of_day: self.default_clock_time() as u64,
            rain,
            thunder,
            sky_kind: dimension.sky_kind,
            light_directions: Some(dimension.light_directions),
            dark_sky_disc: camera[1]
                < if facts.flat_world {
                    dimension.min_y as f64
                } else {
                    63.
                },
            sun_angle: float(A::SunAngle),
            moon_angle: float(A::MoonAngle),
            star_angle: float(A::StarAngle),
            star_brightness: float(A::StarBrightness),
            sunrise_color: rgba(color(A::Sunrise)),
            sun_texture: None,
            moon_texture: None,
            end_sky_texture: None,
            end_flash_texture: None,
            end_flash_intensity: lerp(self.flash.old, self.flash.intensity, self.partial),
            end_flash_x_angle: self.flash.x,
            end_flash_y_angle: self.flash.y,
            ..WorldEnvironment::default()
        };
        if dimension.sky_kind == SkyKind::Overworld {
            result.sun_texture = Some(required_texture(
                content,
                "minecraft:environment/celestial/sun",
            )?);
            let phase = float(A::MoonPhase) as usize;
            const PHASES: [&str; 8] = [
                "minecraft:environment/celestial/moon/full_moon",
                "minecraft:environment/celestial/moon/waning_gibbous",
                "minecraft:environment/celestial/moon/third_quarter",
                "minecraft:environment/celestial/moon/waning_crescent",
                "minecraft:environment/celestial/moon/new_moon",
                "minecraft:environment/celestial/moon/waxing_crescent",
                "minecraft:environment/celestial/moon/first_quarter",
                "minecraft:environment/celestial/moon/waxing_gibbous",
            ];
            result.moon_texture = Some(required_texture(
                content,
                PHASES.get(phase).ok_or("moon phase outside 0..7")?,
            )?);
        } else if dimension.sky_kind == SkyKind::End {
            result.end_sky_texture =
                Some(required_texture(content, "minecraft:environment/end_sky")?);
            result.end_flash_texture = Some(required_texture(
                content,
                "minecraft:environment/celestial/end_flash",
            )?);
        }
        let night_vision = night_vision(facts.night_vision_duration, self.partial);
        setup_fog(
            &mut result,
            &v,
            camera,
            yaw,
            pitch,
            distance,
            view_distance,
            facts,
        );
        if facts.medium == CameraMedium::Atmospheric
            && facts.blindness_duration.is_none()
            && !facts.darkness_present
        {
            let p = camera.map(|v| v.floor() as i32);
            let target = self.rain_fog_target(world, p, weather, rain)?;
            let time = self.client_tick as f64 + self.partial as f64;
            let delta = self.last_fog_time.map_or(0., |old| (time - old).max(0.)) as f32;
            self.last_fog_time = Some(time);
            self.rain_fog += (target - self.rain_fog) * delta * 0.2;
            result.fog_start -= 160. * self.rain_fog;
            result.fog_end = result
                .fog_end
                .min(96.)
                .max(result.fog_end - 256. * self.rain_fog);
            if facts.boss_fog {
                result.fog_start = result.fog_start.min(10.);
                result.fog_end = result.fog_end.min(96.);
                result.sky_fog_end = result.fog_end;
            }
        }
        apply_fog_darkness(&mut result, camera[1], dimension.min_y, facts, night_vision);
        let darkness = facts.darkness_blend * facts.darkness_effect_scale;
        let flash = if dimension.sky_kind == SkyKind::End && !facts.hide_lightning_flashes {
            result.end_flash_intensity / if facts.boss_fog { 3. } else { 1. }
        } else {
            0.
        };
        self.lightmap.update(LightmapInputs {
            sky_factor: float(A::SkyLightFactor) + flash,
            block_factor: self.flicker + 1.4,
            night_vision: if facts.night_vision_duration.is_some() {
                night_vision
            } else if facts.conduit_power {
                facts.water_vision
            } else {
                0.
            },
            darkness: (cos(((facts.entity_tick as f32 - self.partial)
                * std::f32::consts::PI
                * 0.025) as f64)
                * 0.45
                * darkness)
                .max(0.)
                * facts.darkness_effect_scale,
            boss_darkening: facts.boss_darkening,
            brightness: (facts.gamma - darkness).max(0.),
            block_tint: rgb(color(A::BlockTint)),
            sky_color: rgb(color(A::SkyLightColor)),
            ambient_color: rgb(color(A::AmbientColor)),
            night_vision_color: rgb(color(A::NightVisionColor)),
        });
        Ok(result)
    }
}
fn required_texture(content: &Content, name: &str) -> Result<SkyTexture> {
    let t = content
        .texture(name)
        .ok_or_else(|| format!("missing mandatory environment texture {name}"))?;
    Ok(SkyTexture {
        uv: t.uv,
        layer: t.layer,
    })
}
fn nbt_bool(value: &Value, name: &str) -> Result<bool> {
    match &value[name] {
        Value::Bool(v) => Ok(*v),
        Value::Number(v) => v
            .as_i64()
            .map(|v| v != 0)
            .ok_or_else(|| format!("invalid dimension {name}")),
        _ => Err(format!("dimension {name} missing")),
    }
}
fn gray(c: u32, brightness: f32, factor: f32) -> u32 {
    let channels = rgb(c);
    let value =
        (channels[0] * 255. * 0.3 + channels[1] * 255. * 0.59 + channels[2] * 255. * 0.11) as u32;
    let v = (value as f32 * brightness) as u32;
    color_lerp(c, (c & 0xff000000) | (v << 16) | (v << 8) | v, factor)
}
fn apply_weather(values: &mut [Datum; COUNT], rain: f32, thunder: f32) {
    for (level, is_thunder) in [(rain - thunder, false), (thunder, true)] {
        if level <= 0. {
            continue;
        }
        for (attr, modified) in [
            (
                A::SkyColor,
                Datum::Color(gray(
                    values[A::SkyColor as usize].color(),
                    if is_thunder { 0.24 } else { 0.6 },
                    if is_thunder { 0.94 } else { 0.75 },
                )),
            ),
            (
                A::FogColor,
                Datum::Color(multiply(
                    values[A::FogColor as usize].color(),
                    if is_thunder { 0xff3f3f4c } else { 0xff7f7f99 },
                )),
            ),
            (
                A::Sunrise,
                Datum::Color(multiply(
                    values[A::Sunrise as usize].color(),
                    if is_thunder { 0xff3f3f4c } else { 0xff7f7f99 },
                )),
            ),
            (A::StarBrightness, Datum::Float(0.)),
            (
                A::SkyLightFactor,
                Datum::Float(lerp(
                    values[A::SkyLightFactor as usize].float(),
                    0.24,
                    if is_thunder { 0.52734375 } else { 0.3125 },
                )),
            ),
            (
                A::SkyLightColor,
                Datum::Color(weather_light_color(
                    values[A::SkyLightColor as usize].color(),
                    if is_thunder { 134 } else { 79 },
                )),
            ),
        ] {
            let i = attr as usize;
            values[i] = attr.interpolate(values[i], modified, level, false);
        }
    }
}
fn weather_light_color(c: u32, alpha: u32) -> u32 {
    let mut out = c & 0xff000000;
    for shift in [0, 8, 16] {
        let value = if shift == 0 { 255 } else { 122 };
        out |= (value * alpha + (c >> shift & 255) * (255 - alpha)) / 255 << shift;
    }
    out
}
fn night_vision(duration: Option<i32>, partial: f32) -> f32 {
    match duration {
        None => 0.,
        Some(d) if d < 0 || d > 200 => 1.,
        Some(d) => 0.7 + sin(((d as f32 - partial) * std::f32::consts::PI * 0.2) as f64) * 0.3,
    }
}
fn setup_fog(
    out: &mut WorldEnvironment,
    v: &[Datum; COUNT],
    _camera: [f64; 3],
    yaw: f32,
    pitch: f32,
    distance: f32,
    view: u32,
    p: &PlayerEnvironmentFacts,
) {
    let col = |a: A| v[a as usize].color();
    let val = |a: A| v[a as usize].float();
    let (start, end, sky) = match p.medium {
        CameraMedium::Lava => {
            out.fog_color = rgb((-6743808i32) as u32);
            let (a, b) = if p.spectator {
                (-8., distance * 0.5)
            } else if p.fire_resistance {
                (0., 5.)
            } else {
                (0.25, 1.)
            };
            (a, b, b)
        }
        CameraMedium::PowderSnow => {
            out.fog_color = rgb((-6308916i32) as u32);
            let (a, b) = if p.spectator {
                (-8., distance * 0.5)
            } else {
                (0., 2.)
            };
            (a, b, b)
        }
        _ => {
            if p.medium == CameraMedium::Water {
                out.fog_color = rgb(col(A::WaterFogColor));
            } else {
                let mut c = col(A::FogColor);
                if view >= 4 {
                    let direction = if sin((val(A::SunAngle) * 0.017453292) as f64) > 0. {
                        -1.
                    } else {
                        1.
                    };
                    let dot = -yaw.to_radians().sin() * pitch.to_radians().cos() * direction;
                    let sunrise = rgba(col(A::Sunrise));
                    if dot > 0. && sunrise[3] > 0. {
                        c = color_lerp(c, col(A::Sunrise) | 0xff000000, dot * sunrise[3]);
                    }
                }
                let sky = scale_rgb(
                    scale_rgb(
                        col(A::SkyColor),
                        [
                            1. - out.rain * 0.5,
                            1. - out.rain * 0.5,
                            1. - out.rain * 0.4,
                        ],
                    ),
                    [1. - out.thunder * 0.5; 3],
                );
                let mix = 1.
                    - lerp(0.25, 1., (val(A::SkyFogEnd) / 16.).min(view as f32) / 32.)
                        .clamp(0.25, 1.)
                        .powf(0.25);
                out.fog_color = rgb(color_lerp(c, sky, mix));
            }
            if let Some(d) = p.blindness_duration {
                let b = if d < 0 {
                    5.
                } else {
                    lerp(distance, 5., (d as f32 / 20.).min(1.))
                };
                (b * 0.25, b, b * 0.8)
            } else if p.darkness_present {
                let b = lerp(distance, 15., p.darkness_blend);
                (b * 0.75, b, b)
            } else if p.medium == CameraMedium::Water {
                let b = val(A::WaterFogEnd) * p.water_vision.max(0.25);
                (val(A::WaterFogStart), b, b)
            } else {
                (out.fog_start, out.fog_end, out.sky_fog_end)
            }
        }
    };
    out.fog_start = start;
    out.fog_end = end;
    out.sky_fog_end = sky;
}
fn apply_fog_darkness(
    out: &mut WorldEnvironment,
    y: f64,
    min_y: i32,
    p: &PlayerEnvironmentFacts,
    night: f32,
) {
    let mut darkness = 1. - ((y - min_y as f64) * if p.flat_world { 1. } else { 0.03125 }) as f32;
    if let Some(d) = p.blindness_duration {
        darkness = if d < 0 || d > 19 {
            1.
        } else {
            darkness.max(d as f32 / 20.)
        };
    } else {
        darkness = darkness.max(p.darkness_blend);
    }
    if darkness > 0. && p.medium != CameraMedium::Lava && p.medium != CameraMedium::PowderSnow {
        let scale = (1. - darkness.min(1.)).powi(2);
        for c in &mut out.fog_color {
            *c *= scale;
        }
    }
    for (i, c) in out.fog_color.iter_mut().enumerate() {
        *c *= 1. + p.boss_darkening * ([0.7, 0.6, 0.6][i] - 1.);
    }
    let vision = if p.medium == CameraMedium::Water {
        p.water_vision
    } else if !p.darkness_present {
        night
    } else {
        0.
    };
    if out.fog_color.iter().all(|v| *v != 0.) {
        let max = out.fog_color.iter().copied().fold(0f32, f32::max);
        for c in &mut out.fog_color {
            *c = lerp(*c, *c / max, vision);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fractional_rate_negative_rate_and_freeze_step() {
        let mut e = Environment::new();
        e.clocks.insert(
            "c".into(),
            Clock {
                ticks: 10,
                partial: 0.75,
                rate: 0.5,
            },
        );
        e.tick(1);
        assert_eq!(e.clocks["c"].ticks, 11);
        assert_eq!(e.clocks["c"].partial, 0.25);
        e.frozen = true;
        e.tick(2);
        assert_eq!(e.game_time(), 1);
        assert_eq!(e.partial, 1.);
        e.steps = 1;
        e.tick(3);
        assert_eq!(e.game_time(), 2);
        assert_eq!(e.clocks["c"].partial, 0.75);
        e.tick(4);
        assert_eq!(e.game_time(), 2);
        e.clocks.get_mut("c").unwrap().rate = -1.;
        e.frozen = false;
        e.tick(5);
        assert_eq!(e.clocks["c"].ticks, 10);
        assert_eq!(e.clocks["c"].partial, 0.75);
    }
    #[test]
    fn weather_thunder_requires_rain_and_replaces_rain_layer() {
        let mut clear = defaults();
        clear[A::SkyColor as usize] = Datum::Color(0xff80b0ff);
        let mut rain = clear;
        let mut thunder = clear;
        apply_weather(&mut rain, 1., 0.);
        apply_weather(&mut thunder, 1., 1.);
        assert!(
            thunder[A::SkyColor as usize].color() & 255 < rain[A::SkyColor as usize].color() & 255
        );
        assert_eq!(rain[A::SkyLightFactor as usize].float(), 0.7625);
        assert_eq!(thunder[A::SkyLightFactor as usize].float(), 0.5992187);
    }
    #[test]
    fn lava_precedes_blindness_and_water_vision_boundary() {
        let v = defaults();
        let mut out = WorldEnvironment::default();
        let mut f = PlayerEnvironmentFacts {
            medium: CameraMedium::Lava,
            blindness_duration: Some(-1),
            ..Default::default()
        };
        setup_fog(&mut out, &v, [0.; 3], 0., 0., 128., 8, &f);
        assert_eq!((out.fog_start, out.fog_end), (0.25, 1.));
        f.fire_resistance = true;
        setup_fog(&mut out, &v, [0.; 3], 0., 0., 128., 8, &f);
        assert_eq!((out.fog_start, out.fog_end), (0., 5.));
        f.medium = CameraMedium::Water;
        f.blindness_duration = None;
        setup_fog(&mut out, &v, [0.; 3], 0., 0., 128., 8, &f);
        assert_eq!(out.fog_end, 24.);
        f.water_vision = 1.;
        setup_fog(&mut out, &v, [0.; 3], 0., 0., 128., 8, &f);
        assert_eq!(out.fog_end, 96.);
    }
    #[test]
    fn received_timeline_tag_resolution_fails_instead_of_legacy_dimension_defaults() {
        let mut e = Environment::new();
        let d = json!({"min_y":0,"height":256,"has_skylight":true,"has_ceiling":false,"timelines":"#custom:day"});
        assert!(e.activate_dimension("custom:world", &d, 0).is_err());
        e.configure_registry(
            "minecraft:timeline",
            &[(
                "custom:day".into(),
                json!({"clock":"custom:clock","period_ticks":100,"tracks":{}}),
            )],
        )
        .unwrap();
        e.configure_tags(
            "minecraft:timeline",
            &BTreeMap::from([("custom:day".into(), vec![0])]),
        )
        .unwrap();
        e.configure_registry(
            "minecraft:world_clock",
            &[("custom:clock".into(), json!({}))],
        )
        .unwrap();
        e.activate_dimension("custom:world", &d, 0).unwrap();
        assert_eq!(e.dimension.as_ref().unwrap().timeline_ids, vec![0]);
    }
    #[test]
    fn causal_end_flash_preserves_previous_intensity_and_window_boundaries() {
        let mut flash = EndFlash {
            seed: 1,
            offset: 10,
            duration: 100,
            ..Default::default()
        };
        flash.tick(610);
        assert_eq!(flash.intensity, 0.);
        flash.tick(660);
        assert_eq!(flash.intensity, 1.);
        assert_eq!(flash.old, 0.);
        flash.tick(711);
        assert_eq!(flash.intensity, 0.);
        assert_eq!(flash.old, 1.);
    }
    #[test]
    fn darkness_membership_precedes_water_fog_before_blending_starts() {
        let mut out = WorldEnvironment::default();
        let facts = PlayerEnvironmentFacts {
            medium: CameraMedium::Water,
            darkness_present: true,
            darkness_blend: 0.,
            ..Default::default()
        };
        setup_fog(&mut out, &defaults(), [0.; 3], 0., 0., 128., 8, &facts);
        assert_eq!((out.fog_start, out.fog_end), (96., 128.));
    }
}

#[cfg(test)]
mod unloaded_biome_tests {
    use super::*;
    use crate::protocol::world::{Chunk, Palette, SurfaceHeightmap, empty_block_entities};
    use serde_json::json;

    const RED: u32 = 0xffff0000;

    fn environment() -> Environment {
        let mut e = Environment::new();
        e.configure_registry(
            "minecraft:worldgen/biome",
            &[(
                "minecraft:plains".into(),
                json!({"has_precipitation": true, "attributes": {"minecraft:visual/fog_color": "#ff0000"}}),
            )],
        )
        .unwrap();
        e.activate_dimension(
            "minecraft:overworld",
            &json!({"min_y":0,"height":16,"has_skylight":false,"has_ceiling":true,"skybox":"none"}),
            0,
        )
        .unwrap();
        e
    }
    fn chunk() -> Chunk {
        Chunk {
            sections: vec![Arc::new(crate::protocol::world::Section {
                blocks: Palette {
                    bits: 0,
                    values: vec![0],
                    words: Vec::new(),
                    size: 4096,
                },
                biomes: Palette {
                    bits: 0,
                    values: vec![0],
                    words: Vec::new(),
                    size: 64,
                },
                non_empty_block_count: 0,
            })],
            sky: BTreeMap::new(),
            block: BTreeMap::new(),
            block_entities: empty_block_entities(),
            surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
            surface_heightmap_received: false,
        }
    }

    /// Long teleports land the camera among chunks that have not arrived. The
    /// vanilla client keeps rendering with chunk-less biome lookups; a missing
    /// sample must not end the session.
    #[test]
    fn camera_among_unloaded_chunks_yields_neutral_environment() {
        let e = environment();
        let world = World::new(0, 16);
        let values = e.camera_attributes(&world, [518.5, 130., 64.5]).unwrap();
        assert_eq!(values, defaults());
    }

    #[test]
    fn partially_loaded_neighborhood_blends_only_received_biomes() {
        let e = environment();
        let mut world = World::new(0, 16);
        world.insert([0, 0], chunk()).unwrap();
        let values = e.camera_attributes(&world, [8.5, 8., 8.5]).unwrap();
        assert_eq!(values[A::FogColor as usize], Datum::Color(RED));
    }

    #[test]
    fn unloaded_camera_chunk_holds_rain_fog_at_zero() {
        let e = environment();
        let world = World::new(0, 16);
        assert_eq!(
            e.rain_fog_target(&world, [518, 130, 64], true, 1.).unwrap(),
            0.
        );
    }
}
