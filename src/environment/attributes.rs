//! Independently expressed target-26.1.2 attribute arithmetic. Source contracts:
//! EnvironmentAttributes, AttributeTypes, EnvironmentAttributeMap.Entry,
//! KeyframeTrackSampler, EasingType.CubicBezier, ARGB and ColorModifier javap.
use crate::protocol::Result;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Attribute {
    FogColor,
    FogStart,
    FogEnd,
    SkyFogEnd,
    WaterFogColor,
    WaterFogStart,
    WaterFogEnd,
    SkyColor,
    Sunrise,
    SunAngle,
    MoonAngle,
    StarAngle,
    MoonPhase,
    StarBrightness,
    BlockTint,
    SkyLightColor,
    SkyLightFactor,
    NightVisionColor,
    AmbientColor,
    SkyLightLevel,
    WaterEvaporates,
    FastLava,
    CreakingActive,
}
pub(super) const COUNT: usize = 23;
pub(super) const ALL: [Attribute; COUNT] = [
    Attribute::FogColor,
    Attribute::FogStart,
    Attribute::FogEnd,
    Attribute::SkyFogEnd,
    Attribute::WaterFogColor,
    Attribute::WaterFogStart,
    Attribute::WaterFogEnd,
    Attribute::SkyColor,
    Attribute::Sunrise,
    Attribute::SunAngle,
    Attribute::MoonAngle,
    Attribute::StarAngle,
    Attribute::MoonPhase,
    Attribute::StarBrightness,
    Attribute::BlockTint,
    Attribute::SkyLightColor,
    Attribute::SkyLightFactor,
    Attribute::NightVisionColor,
    Attribute::AmbientColor,
    Attribute::SkyLightLevel,
    Attribute::WaterEvaporates,
    Attribute::FastLava,
    Attribute::CreakingActive,
];
impl Attribute {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.strip_prefix("minecraft:").unwrap_or(name) {
            "visual/fog_color" => Self::FogColor,
            "visual/fog_start_distance" => Self::FogStart,
            "visual/fog_end_distance" => Self::FogEnd,
            "visual/sky_fog_end_distance" => Self::SkyFogEnd,
            "visual/water_fog_color" => Self::WaterFogColor,
            "visual/water_fog_start_distance" => Self::WaterFogStart,
            "visual/water_fog_end_distance" => Self::WaterFogEnd,
            "visual/sky_color" => Self::SkyColor,
            "visual/sunrise_sunset_color" => Self::Sunrise,
            "visual/sun_angle" => Self::SunAngle,
            "visual/moon_angle" => Self::MoonAngle,
            "visual/star_angle" => Self::StarAngle,
            "visual/moon_phase" => Self::MoonPhase,
            "visual/star_brightness" => Self::StarBrightness,
            "visual/block_light_tint" => Self::BlockTint,
            "visual/sky_light_color" => Self::SkyLightColor,
            "visual/sky_light_factor" => Self::SkyLightFactor,
            "visual/night_vision_color" => Self::NightVisionColor,
            "visual/ambient_light_color" => Self::AmbientColor,
            "gameplay/sky_light_level" => Self::SkyLightLevel,
            "gameplay/water_evaporates" => Self::WaterEvaporates,
            "gameplay/fast_lava" => Self::FastLava,
            "gameplay/creaking_active" => Self::CreakingActive,
            _ => return None,
        })
    }
    pub fn color(self) -> bool {
        matches!(
            self,
            Self::FogColor
                | Self::WaterFogColor
                | Self::SkyColor
                | Self::Sunrise
                | Self::BlockTint
                | Self::SkyLightColor
                | Self::NightVisionColor
                | Self::AmbientColor
        )
    }
    pub fn angle(self) -> bool {
        matches!(self, Self::SunAngle | Self::MoonAngle | Self::StarAngle)
    }
    pub fn boolean(self) -> bool {
        matches!(
            self,
            Self::WaterEvaporates | Self::FastLava | Self::CreakingActive
        )
    }
    pub fn positional(self) -> bool {
        !matches!(self, Self::MoonPhase | Self::SkyLightLevel | Self::FastLava)
    }
    pub fn default_value(self) -> Datum {
        if self.boolean() {
            return Datum::Bool(false);
        }
        if self.color() {
            return Datum::Color(match self {
                Self::WaterFogColor => 0xff050533,
                Self::BlockTint => 0xffffd88c,
                Self::SkyLightColor => 0xffffffff,
                Self::NightVisionColor => 0xff999999,
                _ => 0,
            });
        }
        Datum::Float(match self {
            Self::FogEnd => 1024.,
            Self::SkyFogEnd => 512.,
            Self::WaterFogStart => -8.,
            Self::WaterFogEnd => 96.,
            Self::SkyLightFactor => 1.,
            Self::SkyLightLevel => 15.,
            _ => 0.,
        })
    }
    pub fn parse(self, v: &Value) -> Result<Datum> {
        if self.boolean() {
            return match v {
                Value::Bool(v) => Ok(Datum::Bool(*v)),
                Value::Number(v) if v.as_i64() == Some(0) => Ok(Datum::Bool(false)),
                Value::Number(v) if v.as_i64() == Some(1) => Ok(Datum::Bool(true)),
                _ => Err("environment boolean must be bool/byte".into()),
            };
        }
        if self.color() {
            return Ok(Datum::Color(parse_color(v, self == Self::Sunrise)?));
        }
        if self == Self::MoonPhase {
            let phases = [
                "full_moon",
                "waning_gibbous",
                "third_quarter",
                "waning_crescent",
                "new_moon",
                "waxing_crescent",
                "first_quarter",
                "waxing_gibbous",
            ];
            return phases
                .iter()
                .position(|n| Some(*n) == v.as_str())
                .map(|i| Datum::Float(i as f32))
                .ok_or_else(|| "unknown moon phase".into());
        }
        Ok(Datum::Float(number(v)?))
    }
    pub fn interpolate(self, a: Datum, b: Datum, t: f32, partial: bool) -> Datum {
        match (a, b) {
            (Datum::Bool(a), Datum::Bool(_)) => Datum::Bool(a),
            (Datum::Color(a), Datum::Color(b)) => Datum::Color(color_lerp(a, b, t)),
            (Datum::Float(a), Datum::Float(b)) => {
                if self == Self::MoonPhase {
                    return Datum::Float(if partial || t >= 1. { b } else { a });
                }
                if partial && self.angle() {
                    let d = (b - a + 180.).rem_euclid(360.) - 180.;
                    return Datum::Float(if d.abs() >= 90. { b } else { a + t * d });
                }
                Datum::Float(lerp(a, b, t))
            }
            _ => unreachable!("attribute type fixed at parse"),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Datum {
    Float(f32),
    Color(u32),
    Bool(bool),
}
impl Datum {
    pub fn boolean(self) -> bool {
        if let Self::Bool(v) = self {
            v
        } else {
            unreachable!("attribute boolean type checked at parse")
        }
    }
    pub fn float(self) -> f32 {
        if let Self::Float(v) = self {
            v
        } else {
            unreachable!()
        }
    }
    pub fn color(self) -> u32 {
        if let Self::Color(v) = self {
            v
        } else {
            unreachable!()
        }
    }
}
pub(super) fn defaults() -> [Datum; COUNT] {
    std::array::from_fn(|i| ALL[i].default_value())
}
#[derive(Clone, Copy, Debug)]
enum Operation {
    Override,
    Add,
    Subtract,
    Multiply,
    Minimum,
    Maximum,
    AlphaBlend,
    BlendGray,
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Modifier {
    operation: Operation,
    argument: Datum,
    alpha: f32,
}
impl Modifier {
    pub fn parse(attr: Attribute, v: &Value) -> Result<Self> {
        let (op, arg) = if let Some(op) = v.get("modifier").and_then(Value::as_str) {
            (
                op,
                v.get("argument")
                    .ok_or("attribute modifier missing argument")?,
            )
        } else {
            ("override", v)
        };
        Self::argument(attr, op, arg)
    }
    fn argument(attr: Attribute, op: &str, arg: &Value) -> Result<Self> {
        let operation = match op.strip_prefix("minecraft:").unwrap_or(op) {
            "override" => Operation::Override,
            "add" => Operation::Add,
            "subtract" => Operation::Subtract,
            "multiply" => Operation::Multiply,
            "minimum" => Operation::Minimum,
            "maximum" => Operation::Maximum,
            "alpha_blend" => Operation::AlphaBlend,
            "blend_to_gray" => Operation::BlendGray,
            "and" if attr.boolean() => Operation::And,
            "nand" if attr.boolean() => Operation::Nand,
            "or" if attr.boolean() => Operation::Or,
            "nor" if attr.boolean() => Operation::Nor,
            "xor" if attr.boolean() => Operation::Xor,
            "xnor" if attr.boolean() => Operation::Xnor,
            _ => return Err(format!("unknown environment modifier {op}")),
        };
        let (argument, alpha) = if matches!(operation, Operation::BlendGray) {
            (
                Datum::Float(number(&arg["brightness"])?),
                number(&arg["factor"])?,
            )
        } else if matches!(operation, Operation::AlphaBlend) && !attr.color() {
            if arg.is_number() {
                (attr.parse(arg)?, 1.)
            } else {
                (
                    attr.parse(&arg["value"])?,
                    arg.get("alpha").map(number).transpose()?.unwrap_or(1.),
                )
            }
        } else {
            (attr.parse(arg)?, 1.)
        };
        if matches!(operation, Operation::BlendGray) && !attr.color() {
            return Err("gray modifier requires color".into());
        }
        if attr.boolean()
            && !matches!(
                operation,
                Operation::Override
                    | Operation::And
                    | Operation::Nand
                    | Operation::Or
                    | Operation::Nor
                    | Operation::Xor
                    | Operation::Xnor
            )
        {
            return Err("boolean attribute requires boolean modifier".into());
        }
        if attr == Attribute::MoonPhase && !matches!(operation, Operation::Override) {
            return Err("moon phase only accepts override".into());
        }
        Ok(Self {
            operation,
            argument,
            alpha,
        })
    }
    pub fn apply(self, base: Datum) -> Datum {
        match (base, self.argument) {
            (Datum::Bool(a), Datum::Bool(b)) => Datum::Bool(match self.operation {
                Operation::Override => b,
                Operation::And => a && b,
                Operation::Nand => !(a && b),
                Operation::Or => a || b,
                Operation::Nor => !(a || b),
                Operation::Xor => a ^ b,
                Operation::Xnor => a == b,
                _ => unreachable!("boolean modifier checked at parse"),
            }),
            (Datum::Float(a), Datum::Float(b)) => Datum::Float(match self.operation {
                Operation::Override => b,
                Operation::Add => a + b,
                Operation::Subtract => a - b,
                Operation::Multiply => a * b,
                Operation::Minimum => a.min(b),
                Operation::Maximum => a.max(b),
                Operation::AlphaBlend => lerp(a, b, self.alpha),
                Operation::BlendGray => unreachable!(),
                _ => unreachable!("numeric modifier checked at parse"),
            }),
            (Datum::Color(a), Datum::Color(b)) => Datum::Color(match self.operation {
                Operation::Override => b,
                Operation::Multiply => multiply(a, b),
                Operation::AlphaBlend => alpha_blend(a, b),
                Operation::Add => color_channels(a, b, |x, y| (x + y).min(255)),
                Operation::Subtract => color_channels(a, b, |x, y| x.saturating_sub(y)),
                Operation::Minimum => color_channels(a, b, u32::min),
                Operation::Maximum => color_channels(a, b, u32::max),
                Operation::BlendGray => unreachable!(),
                _ => unreachable!("color modifier checked at parse"),
            }),
            (Datum::Color(a), Datum::Float(b)) => {
                let [r, g, bl] = rgb(a);
                let gray = ((r * 255. * 0.3 + g * 255. * 0.59 + bl * 255. * 0.11) as u32) as f32;
                let c = (gray * b) as u32;
                Datum::Color(color_lerp(
                    a,
                    (a & 0xff000000) | (c << 16) | (c << 8) | c,
                    self.alpha,
                ))
            }
            _ => unreachable!("modifier types validated"),
        }
    }
}
pub(super) type AttributeMap = [Option<Modifier>; COUNT];
pub(super) fn parse_map(value: &Value) -> Result<AttributeMap> {
    let mut out = [None; COUNT];
    if value.is_null() {
        return Ok(out);
    }
    for (key, v) in value
        .as_object()
        .ok_or("environment attributes must be map")?
    {
        if let Some(a) = Attribute::from_name(key) {
            out[a as usize] = Some(Modifier::parse(a, v)?);
        }
    }
    Ok(out)
}
pub(super) fn apply_map(map: &AttributeMap, mut base: [Datum; COUNT]) -> [Datum; COUNT] {
    for i in 0..COUNT {
        if let Some(m) = map[i] {
            base[i] = m.apply(base[i]);
        }
    }
    base
}
#[derive(Clone, Debug)]
pub(super) struct Track {
    pub attribute: Attribute,
    keyframes: Vec<(i64, Modifier)>,
    ease: Ease,
}
#[derive(Clone, Copy, Debug)]
enum Ease {
    Linear,
    Constant,
    Bezier([f32; 4]),
}
impl Track {
    pub fn parse(attribute: Attribute, value: &Value, period: Option<i64>) -> Result<Self> {
        let ease = match value.get("ease") {
            None => Ease::Linear,
            Some(v) if v.as_str() == Some("linear") => Ease::Linear,
            Some(v) if v.as_str() == Some("constant") => Ease::Constant,
            Some(v) => {
                let a = v["cubic_bezier"]
                    .as_array()
                    .ok_or("unknown timeline easing")?;
                if a.len() != 4 {
                    return Err("bezier needs four controls".into());
                }
                Ease::Bezier([
                    number(&a[0])?,
                    number(&a[1])?,
                    number(&a[2])?,
                    number(&a[3])?,
                ])
            }
        };
        let operation = value
            .get("modifier")
            .and_then(Value::as_str)
            .unwrap_or("override")
            .to_owned();
        let frames = value["keyframes"]
            .as_array()
            .ok_or("timeline keyframes missing")?;
        if frames.is_empty() || frames.len() > 4096 {
            return Err("timeline keyframe bound".into());
        }
        let mut keyframes = Vec::with_capacity(frames.len());
        for f in frames {
            let ticks = f["ticks"].as_i64().ok_or("keyframe ticks missing")?;
            if keyframes.last().is_some_and(|(t, _)| ticks < *t)
                || period.is_some_and(|p| ticks < 0 || ticks > p)
            {
                return Err("unordered/out of period keyframe".into());
            }
            let arg = f.get("value").ok_or("keyframe value missing")?;
            keyframes.push((ticks, Modifier::argument(attribute, &operation, arg)?));
        }
        Ok(Self {
            attribute,
            keyframes,
            ease,
        })
    }
    pub fn apply(&self, base: Datum, ticks: i64, period: Option<i64>) -> Result<Datum> {
        let frames = &self.keyframes;
        let t = period.map_or(ticks, |p| ticks.rem_euclid(p));
        let first = &frames[0];
        let last = &frames[frames.len() - 1];
        let (a, at, b, bt) = if frames.len() == 1 {
            (first, first.0, first, first.0)
        } else if t < first.0 {
            if let Some(p) = period {
                (last, last.0 - p, first, first.0)
            } else {
                (first, first.0, first, first.0)
            }
        } else if let Some(i) = frames.windows(2).position(|w| t < w[1].0) {
            (&frames[i], frames[i].0, &frames[i + 1], frames[i + 1].0)
        } else if let Some(p) = period {
            (last, last.0, first, first.0 + p)
        } else {
            (last, last.0, last, last.0)
        };
        let m1 = a.1;
        let m2 = b.1;
        let mut m = m1;
        if t >= bt {
            m = m2;
        } else if t > at {
            let x = (t - at) as f32 / (bt - at) as f32;
            let x = match self.ease {
                Ease::Linear => x,
                Ease::Constant => 0.,
                Ease::Bezier(c) => bezier(c, x),
            };
            m.argument = self
                .attribute
                .interpolate(m1.argument, m2.argument, x, false);
            m.alpha = lerp(m1.alpha, m2.alpha, x);
        }
        Ok(m.apply(base))
    }
}
fn bezier(c: [f32; 4], x: f32) -> f32 {
    let curve = |a: f32, b: f32, t: f32| {
        ((3. * a - 3. * b + 1.) * t + (-6. * a + 3. * b)) * t * t + 3. * a * t
    };
    let mut t = x;
    for _ in 0..4 {
        let gradient =
            (3. * (3. * c[0] - 3. * c[2] + 1.) * t + 2. * (-6. * c[0] + 3. * c[2])) * t + 3. * c[0];
        if gradient < 1e-5 {
            break;
        }
        t -= (curve(c[0], c[2], t) - x) / gradient;
    }
    curve(c[1], c[3], t)
}
pub(super) fn number(v: &Value) -> Result<f32> {
    v.as_f64()
        .filter(|v| v.is_finite() && v.abs() <= f32::MAX as f64)
        .map(|v| v as f32)
        .ok_or_else(|| "environment value must be finite number".into())
}
fn parse_color(v: &Value, argb: bool) -> Result<u32> {
    if let Some(n) = v.as_i64() {
        return Ok(n as u32);
    }
    let s = v
        .as_str()
        .ok_or("environment color must be integer/hex")?
        .strip_prefix('#')
        .ok_or("environment hex color missing #")?;
    if s.len() != if argb { 8 } else { 6 } {
        return Err("environment color width".into());
    }
    u32::from_str_radix(s, 16)
        .map(|n| if argb { n } else { 0xff000000 | n })
        .map_err(|_| "invalid environment color".into())
}
pub(super) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}
pub(super) fn rgb(c: u32) -> [f32; 3] {
    [
        (c >> 16 & 255) as f32 / 255.,
        (c >> 8 & 255) as f32 / 255.,
        (c & 255) as f32 / 255.,
    ]
}
pub(super) fn rgba(c: u32) -> [f32; 4] {
    let r = rgb(c);
    [r[0], r[1], r[2], (c >> 24) as f32 / 255.]
}
pub(super) fn color_lerp(a: u32, b: u32, t: f32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let x = (a >> shift & 255) as i32;
        let y = (b >> shift & 255) as i32;
        out |= ((x + (t * (y - x) as f32).floor() as i32) as u32 & 255) << shift;
    }
    out
}
pub(super) fn multiply(a: u32, b: u32) -> u32 {
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        out |= ((a >> shift & 255) * (b >> shift & 255) / 255) << shift;
    }
    out
}
fn color_channels(a: u32, b: u32, op: fn(u32, u32) -> u32) -> u32 {
    let mut out = a & 0xff000000;
    for shift in [0, 8, 16] {
        out |= op(a >> shift & 255, b >> shift & 255) << shift;
    }
    out
}
pub(super) fn scale_rgb(c: u32, scale: [f32; 3]) -> u32 {
    let mut out = c & 0xff000000;
    for (i, shift) in [16, 8, 0].into_iter().enumerate() {
        out |= (((c >> shift & 255) as f32 * scale[i]) as u32).min(255) << shift;
    }
    out
}
fn alpha_blend(a: u32, b: u32) -> u32 {
    let aa = a >> 24;
    let ba = b >> 24;
    let alpha = ba + aa * (255 - ba) / 255;
    if alpha == 0 {
        return 0;
    }
    let mut out = alpha << 24;
    for shift in [0, 8, 16] {
        out |= ((b >> shift & 255) * ba + (a >> shift & 255) * (alpha - ba)) / alpha << shift;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn duplicate_time_celestial_track_wraps_without_extra_moon_rotation() {
        let track = Track::parse(
            Attribute::MoonAngle,
            &json!({"keyframes":[{"ticks":6,"value":540.},{"ticks":6,"value":180.}]}),
            Some(24),
        )
        .unwrap();
        assert_eq!(
            track.apply(Datum::Float(0.), 6, Some(24)).unwrap(),
            Datum::Float(180.)
        );
        assert_eq!(
            track.apply(Datum::Float(0.), 18, Some(24)).unwrap(),
            Datum::Float(360.)
        );
        assert_eq!(
            track.apply(Datum::Float(0.), -6, Some(24)).unwrap(),
            Datum::Float(360.)
        );
    }
    #[test]
    fn dimension_biome_timeline_precedence_and_color_quantization() {
        let d = parse_map(&json!({"minecraft:visual/sky_color":"#80c0ff"})).unwrap();
        let b = parse_map(
            &json!({"minecraft:visual/sky_color":{"modifier":"multiply","argument":"#ff8000"}}),
        )
        .unwrap();
        let out = apply_map(&b, apply_map(&d, defaults()));
        assert_eq!(out[Attribute::SkyColor as usize], Datum::Color(0xff806000));
        assert_eq!(color_lerp(0xff000000, 0xffffffff, 0.5), 0xff7f7f7f);
        let track = Track::parse(
            Attribute::SkyColor,
            &json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":"#80ff80"}]}),
            None,
        )
        .unwrap();
        assert_eq!(
            track
                .apply(out[Attribute::SkyColor as usize], 0, None)
                .unwrap(),
            Datum::Color(0xff406000)
        );
    }
    #[test]
    fn moon_phase_is_step_and_angle_partial_crosses_zero() {
        assert_eq!(
            Attribute::MoonPhase.interpolate(Datum::Float(0.), Datum::Float(1.), 0.5, false),
            Datum::Float(0.)
        );
        assert_eq!(
            Attribute::MoonPhase.interpolate(Datum::Float(0.), Datum::Float(1.), 0., true),
            Datum::Float(1.)
        );
        assert_eq!(
            Attribute::SunAngle.interpolate(Datum::Float(359.), Datum::Float(1.), 0.5, true),
            Datum::Float(360.)
        );
    }
}
