//! Independently expressed pinned biome precipitation temperature arithmetic.
//! Source: ignored point-environment{,-detail}.javap, Biome/TemperatureModifier,
//! PerlinSimplexNoise and existing pinned Simplex2/JavaRandom primitives.
use crate::protocol::{Result, content::Simplex2, models::JavaRandom};
use serde_json::Value;
use std::sync::LazyLock;

struct Noise {
    temperature: Simplex2,
    info: Simplex2,
    frozen: [Simplex2; 3],
}
static NOISE: LazyLock<Noise> = LazyLock::new(|| {
    let mut random = JavaRandom::new(3456);
    Noise {
        temperature: Simplex2::new(1234),
        info: Simplex2::new(2345),
        frozen: std::array::from_fn(|_| Simplex2::from_random(&mut random)),
    }
});
fn frozen_temperature(position: [i32; 3], temperature: f32) -> f32 {
    let x = f64::from(position[0]);
    let z = f64::from(position[2]);
    // Octaves [0,-1,-2], highest-frequency input 1 and value weight 1/7.
    let mut value = 0.;
    let mut frequency = 1.;
    let mut weight = 1. / 7.;
    for noise in &NOISE.frozen {
        value += noise.value(x * 0.05 * frequency, z * 0.05 * frequency) * weight;
        frequency /= 2.;
        weight *= 2.;
    }
    if value * 7. + NOISE.info.value(x * 0.2, z * 0.2) < 0.3
        && NOISE.info.value(x * 0.09, z * 0.09) < 0.8
    {
        0.2
    } else {
        temperature
    }
}
pub(crate) fn temperature(biome: &Value, position: [i32; 3], sea_level: i32) -> Result<f32> {
    let mut temperature = super::attributes::number(&biome["temperature"])?;
    let modifier = biome
        .get("temperature_modifier")
        .map(|value| {
            value
                .as_str()
                .ok_or("biome temperature modifier must be string")
        })
        .transpose()?
        .unwrap_or("none");
    match modifier {
        "none" => {}
        "frozen" => temperature = frozen_temperature(position, temperature),
        other => return Err(format!("unknown biome temperature modifier {other}")),
    }
    let threshold = sea_level.wrapping_add(17);
    if position[1] > threshold {
        // Source performs integer->float and division in f32 BEFORE widening.
        let noise = (NOISE.temperature.value(
            f64::from(position[0] as f32 / 8.),
            f64::from(position[2] as f32 / 8.),
        ) * 8.) as f32;
        temperature -= (noise + position[1] as f32 - threshold as f32) * 0.05 / 40.;
    }
    Ok(temperature)
}
pub(crate) fn has_precipitation(biome: &Value) -> Result<bool> {
    match &biome["has_precipitation"] {
        Value::Bool(value) => Ok(*value),
        Value::Number(value) if value.as_i64() == Some(0) => Ok(false),
        Value::Number(value) if value.as_i64() == Some(1) => Ok(true),
        _ => Err("biome precipitation missing/invalid".into()),
    }
}
pub(crate) fn rains(biome: &Value, position: [i32; 3], sea_level: i32) -> Result<bool> {
    Ok(has_precipitation(biome)? && temperature(biome, position, sea_level)? >= 0.15)
}
