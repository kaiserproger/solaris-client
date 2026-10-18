//! 26.1.2 Mth.sin/cos use a shared 65536-entry table, not continuous trigonometry.
use std::sync::LazyLock;
const QUANTIZATION: f64 = 10430.378350470453;
static SIN: LazyLock<[f32; 65536]> =
    LazyLock::new(|| std::array::from_fn(|i| (i as f64 / QUANTIZATION).sin() as f32));
pub(crate) fn sin(angle: f64) -> f32 {
    SIN[((angle * QUANTIZATION) as i64 & 65535) as usize]
}
pub(crate) fn cos(angle: f64) -> f32 {
    SIN[((angle * QUANTIZATION + 16384.) as i64 & 65535) as usize]
}
static ATAN: LazyLock<[(f64, f64); 257]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let asin = libm::asin(index as f64 / 256.0);
        (asin, libm::cos(asin))
    })
});
pub(crate) fn atan2(mut y: f64, mut x: f64) -> f64 {
    let length_squared = x * x + y * y;
    if length_squared.is_nan() {
        return f64::NAN;
    }
    let negative_y = y < 0.0;
    if negative_y {
        y = -y;
    }
    let negative_x = x < 0.0;
    if negative_x {
        x = -x;
    }
    let swapped = y > x;
    if swapped {
        std::mem::swap(&mut x, &mut y);
    }
    let half = 0.5 * length_squared;
    let bits = 6910469410427058090i64.wrapping_sub((length_squared.to_bits() as i64) >> 1);
    let estimate = f64::from_bits(bits as u64);
    let inverse = estimate * (1.5 - half * estimate * estimate);
    x *= inverse;
    y *= inverse;
    let bias = f64::from_bits(4805340802404319232);
    let rounded = bias + y;
    let (asin, cos) = ATAN[rounded.to_bits() as u32 as usize];
    let quantized = rounded - bias;
    let discrepancy = y * cos - x * quantized;
    let correction = (6.0 + discrepancy * discrepancy) * discrepancy * 0.16666666666666666;
    let mut angle = asin + correction;
    if swapped {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if negative_x {
        angle = std::f64::consts::PI - angle;
    }
    if negative_y {
        angle = -angle;
    }
    angle
}
pub(super) fn view(pitch: f32, yaw: f32) -> [f64; 3] {
    let p = f64::from(pitch * 0.017453292);
    let y = f64::from(-yaw * 0.017453292);
    let c = cos(p);
    [
        f64::from(sin(y) * c),
        f64::from(-sin(p)),
        f64::from(cos(y) * c),
    ]
}
