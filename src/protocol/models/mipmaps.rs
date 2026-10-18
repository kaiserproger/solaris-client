//! Sprite-local 26.1.2 mip processing, before aligned atlas placement.
use super::super::{Result, content::Image};
use serde_json::Value;
use std::{collections::VecDeque, sync::LazyLock};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    Mean,
    Cutout,
    StrictCutout,
    DarkCutout,
}
impl Strategy {
    pub fn parse(metadata: &Value, transparent: bool) -> Result<Self> {
        Ok(
            match metadata["mipmap_strategy"].as_str().unwrap_or("auto") {
                "auto" => {
                    if transparent {
                        Self::Cutout
                    } else {
                        Self::Mean
                    }
                }
                "mean" => Self::Mean,
                "cutout" => Self::Cutout,
                "strict_cutout" => Self::StrictCutout,
                "dark_cutout" => Self::DarkCutout,
                other => return Err(format!("unsupported sprite mipmap strategy {other}")),
            },
        )
    }
    fn cutoff(self) -> Option<f32> {
        match self {
            Self::Mean => None,
            Self::StrictCutout => Some(0.3),
            _ => Some(0.5),
        }
    }
}
struct Transfer {
    to_linear: [u16; 256],
    to_srgb: [u8; 1024],
}
static TRANSFER: LazyLock<Transfer> = LazyLock::new(|| Transfer {
    to_linear: std::array::from_fn(|i| {
        let value = i as f32 / 255.;
        let linear = if value >= 0.04045 {
            ((f64::from(value) + 0.055) / 1.055).powf(2.4) as f32
        } else {
            value / 12.92
        };
        (linear * 1023.).round() as u16
    }),
    to_srgb: std::array::from_fn(|i| {
        let value = i as f32 / 1023.;
        let encoded = if value >= 0.0031308 {
            (1.055 * f64::from(value).powf(1. / 2.4) - 0.055) as f32
        } else {
            value * 12.92
        };
        (encoded * 255.).round() as u8
    }),
});
fn transfer() -> &'static Transfer {
    &TRANSFER
}
fn solidify(image: &mut Image) {
    let w = image.width as usize;
    let h = image.height as usize;
    let mut distance = vec![u32::MAX; w * h];
    let mut colors = vec![[0u8; 3]; w * h];
    let mut queue = VecDeque::with_capacity(w * h);
    for x in 0..w {
        for y in 0..h {
            let i = y * w + x;
            if image.rgba[i * 4 + 3] != 0 {
                distance[i] = 0;
                colors[i].copy_from_slice(&image.rgba[i * 4..i * 4 + 3]);
                queue.push_back(i);
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        let x = i % w;
        let y = i / w;
        for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                continue;
            }
            let n = ny as usize * w + nx as usize;
            if distance[n] > distance[i] + 1 {
                distance[n] = distance[i] + 1;
                colors[n] = colors[i];
                queue.push_back(n);
            }
        }
    }
    for (i, pixel) in image.rgba.chunks_exact_mut(4).enumerate() {
        if pixel[3] == 0 {
            pixel[..3].copy_from_slice(&colors[i]);
        }
    }
}
fn dark_fill(image: &mut Image) {
    let mut darkest = [255u8; 3];
    let mut minimum = u32::MAX;
    for x in 0..image.width {
        for y in 0..image.height {
            let i = ((y * image.width + x) * 4) as usize;
            let pixel = &image.rgba[i..i + 4];
            let sum = pixel[..3].iter().map(|v| u32::from(*v)).sum::<u32>();
            if pixel[3] != 0 && sum < minimum {
                minimum = sum;
                darkest.copy_from_slice(&pixel[..3]);
            }
        }
    }
    darkest = darkest.map(|v| (u16::from(v) * 3 / 4) as u8);
    for pixel in image.rgba.chunks_exact_mut(4) {
        if pixel[3] == 0 {
            pixel[..3].copy_from_slice(&darkest);
        }
    }
}
fn coverage(image: &Image, cutoff: f32, scale: f32) -> f32 {
    let w = image.width;
    let h = image.height;
    let mut covered = 0.;
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let alpha = |dx, dy| {
                ((image.rgba[(((y + dy) * w + x + dx) * 4 + 3) as usize] as f32 / 255.) * scale)
                    .clamp(0., 1.)
            };
            let a = alpha(0, 0);
            let b = alpha(1, 0);
            let c = alpha(0, 1);
            let d = alpha(1, 1);
            let mut hits = 0.;
            for iy in 0..4 {
                let v = (iy as f32 + 0.5) / 4.;
                for ix in 0..4 {
                    let u = (ix as f32 + 0.5) / 4.;
                    let value =
                        a * (1. - u) * (1. - v) + b * u * (1. - v) + c * (1. - u) * v + d * u * v;
                    if value > cutoff {
                        hits += 1.;
                    }
                }
            }
            covered += hits / 16.;
        }
    }
    covered / (w.saturating_sub(1) * h.saturating_sub(1)) as f32
}
fn scale_coverage(image: &mut Image, target: f32, cutoff: f32, bias: f32) {
    let (mut low, mut high, mut scale, mut best, mut error) = (0f32, 4f32, 1f32, 1f32, f32::MAX);
    for _ in 0..5 {
        let actual = coverage(image, cutoff, scale);
        let delta = (actual - target).abs();
        if delta < error {
            error = delta;
            best = scale;
        }
        if actual < target {
            low = scale;
        } else if actual > target {
            high = scale;
        } else {
            break;
        }
        scale = (low + high) * 0.5;
    }
    for pixel in image.rgba.chunks_exact_mut(4) {
        pixel[3] =
            ((pixel[3] as f32 / 255. * best + bias + 0.025).clamp(0., 1.) * 255.).floor() as u8;
    }
}
fn downsample(image: &Image, strategy: Strategy) -> Image {
    let width = (image.width >> 1).max(1);
    let height = (image.height >> 1).max(1);
    let mut rgba = vec![0; (width * height * 4) as usize];
    let table = transfer();
    for y in 0..height {
        for x in 0..width {
            let offsets = std::array::from_fn::<_, 4, _>(|i| {
                (((y * 2 + (i / 2) as u32).min(image.height - 1) * image.width
                    + (x * 2 + (i % 2) as u32).min(image.width - 1))
                    * 4) as usize
            });
            let dst = ((y * width + x) * 4) as usize;
            for channel in 0..4 {
                if channel == 3 && strategy != Strategy::DarkCutout {
                    rgba[dst + 3] = (offsets
                        .iter()
                        .map(|i| u32::from(image.rgba[i + 3]))
                        .sum::<u32>()
                        / 4) as u8;
                    continue;
                }
                let sum = offsets
                    .iter()
                    .filter(|i| strategy != Strategy::DarkCutout || image.rgba[**i + 3] != 0)
                    .map(|i| u32::from(table.to_linear[image.rgba[i + channel] as usize]))
                    .sum::<u32>();
                rgba[dst + channel] = table.to_srgb[(sum / 4) as usize];
            }
        }
    }
    Image {
        width,
        height,
        rgba,
    }
}
pub(crate) fn generate(
    mut image: Image,
    strategy: Strategy,
    bias: f32,
    item: bool,
) -> Result<Vec<Image>> {
    if !bias.is_finite() || !(-1. ..=1.).contains(&bias) {
        return Err("sprite alpha cutoff bias bound".into());
    }
    if !item {
        match strategy {
            Strategy::Cutout | Strategy::StrictCutout => solidify(&mut image),
            Strategy::DarkCutout => dark_fill(&mut image),
            Strategy::Mean => {}
        }
    }
    let target = strategy.cutoff().map(|cutoff| coverage(&image, cutoff, 1.));
    let mut levels = Vec::with_capacity(5);
    levels.push(image);
    for _ in 1..=4 {
        let mut next = downsample(levels.last().unwrap(), strategy);
        if let (Some(target), Some(cutoff)) = (target, strategy.cutoff()) {
            scale_coverage(&mut next, target, cutoff, bias);
        }
        levels.push(next);
    }
    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_mips_preserve_srgb_linear_mean_not_encoded_average() {
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
        };
        let levels = generate(image, Strategy::Mean, 0., false).unwrap();
        // Vanilla `ARGB.linearChannelMean` truncates the summed 10-bit table
        // entries (`(t[a]+t[b]+t[c]+t[d])/4`, idiv) before the reverse lookup:
        // (0+1023+0+1023)/4 = 511 -> 187. The exact float linear mean would be
        // 188; vanilla javap (`ARGB.javap`, `MipmapGenerator.javap`) pins the
        // truncating table path, so this pins 187, not 188.
        assert_eq!(levels[1].rgba, [187, 187, 187, 255]);
    }
    #[test]
    fn solidification_retains_zero_alpha_without_black_border() {
        let mut image = Image {
            width: 3,
            height: 1,
            rgba: vec![255, 32, 16, 255, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        solidify(&mut image);
        assert_eq!(&image.rgba[4..], [255, 32, 16, 0, 255, 32, 16, 0]);
    }
}
