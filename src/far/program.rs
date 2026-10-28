//! Far-terrain generation program (`solaris:lod/program`), sent only when the
//! server operator opted in (`[lod] share_terrain_keys`): decoding and the
//! CPU evaluator. The format and the reference semantics are documented in
//! the server's `mc_worldgen::vanilla_density::program`; this evaluator gives
//! the server's `lod_column` answer bit for bit (the parity fixture test),
//! and the GPU evaluator (`render/far_generate.wgsl`) the same in f32.
use crate::protocol::{Reader, Result};

pub(crate) const VERSION: u16 = 1;
/// Instruction ops whose arguments name registers (bit `i` = argument `i`).
const REGISTER_ARGS: [u8; 21] = [
    0, 3, 3, 3, 3, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0b1110, 0b10, 0b111, 1, 0,
];
/// Largest code the GPU register file holds.
pub(crate) const MAX_REGISTERS: usize = 2048;

pub(crate) struct Octave {
    pub p: [u8; 256],
    pub offset: [f64; 3],
}

pub(crate) struct Perlin {
    pub input: f64,
    pub value: f64,
    /// Amplitude and octave per level (`None` = zero amplitude level).
    pub levels: Vec<(f64, Option<u32>)>,
}

pub(crate) struct Normal {
    pub value_factor: f64,
    pub first: Perlin,
    pub second: Perlin,
}

pub(crate) struct Blended {
    /// 16 min, 16 max, 8 main octaves.
    pub octaves: [u32; 40],
    /// xz, y (both times 684.412), xz_factor, y_factor, smear.
    pub scales: [f64; 5],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Inst {
    pub op: u8,
    pub args: [u32; 4],
    pub constants: [f64; 2],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Point {
    pub location: f32,
    pub derivative: f32,
    /// `value` is a register index, else `f32` bits.
    pub register: bool,
    pub value: u32,
}

#[derive(Default)]
pub(crate) struct Code {
    pub insts: Vec<Inst>,
    pub points: Vec<Point>,
}

pub(crate) struct Program {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub cell_width: i32,
    pub cell_height: i32,
    pub octaves: Vec<Octave>,
    pub normals: Vec<Normal>,
    pub blended: Vec<Blended>,
    pub surface: Code,
    /// Surface instructions `[0, flat_len)` run once per column at y 0.
    pub flat_len: u32,
    pub surface_root: u32,
    pub climate: Code,
    /// Temperature, vegetation, continents, erosion, depth, ridges.
    pub climate_roots: [u32; 6],
    pub biomes: Vec<String>,
    /// Quantized climate bounds and biome name index, in search order.
    pub rows: Vec<([[i64; 2]; 7], u16)>,
}

/// One generated column: top solid block and biome name index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Column {
    pub height: i32,
    pub biome: u16,
}

struct Le<'a>(Reader<'a>);
impl Le<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.0.take(N)?.try_into().expect("exact length"))
    }
    fn u8(&mut self) -> Result<u8> {
        self.0.u8()
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.bytes()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.bytes()?))
    }
    /// A table length, bounded so a hostile payload cannot reserve much.
    fn len(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("far terrain program table too large".into());
        }
        Ok(n)
    }
}

impl Program {
    /// Decodes and validates an uncompressed program.
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        let mut r = Le(Reader::new(bytes));
        if &r.bytes::<4>()? != b"SLOD" {
            return Err("not a far terrain program".into());
        }
        if r.u16()? != VERSION {
            return Err("unsupported far terrain program version".into());
        }
        let [min_y, height, sea_level, cell_width, cell_height] =
            [r.i32()?, r.i32()?, r.i32()?, r.i32()?, r.i32()?];
        if !(1..=4096).contains(&height)
            || !(1..=64).contains(&cell_width)
            || !(1..=64).contains(&cell_height)
            || !(-4096..=4096).contains(&min_y)
        {
            return Err("invalid far terrain program geometry".into());
        }
        let mut octaves = Vec::new();
        for _ in 0..r.len(4096)? {
            let p = r.bytes::<256>()?;
            octaves.push(Octave {
                p,
                offset: [r.f64()?, r.f64()?, r.f64()?],
            });
        }
        let octave = |id: u32| -> Result<u32> {
            if (id as usize) < octaves.len() {
                Ok(id)
            } else {
                Err("far terrain octave out of range".into())
            }
        };
        let mut normals = Vec::new();
        for _ in 0..r.len(1024)? {
            let value_factor = r.f64()?;
            let mut perlin = || -> Result<Perlin> {
                let input = r.f64()?;
                let value = r.f64()?;
                let mut levels = Vec::new();
                for _ in 0..r.len(64)? {
                    let amplitude = r.f64()?;
                    let id = r.u32()?;
                    levels.push((amplitude, (id != u32::MAX).then(|| octave(id)).transpose()?));
                }
                Ok(Perlin {
                    input,
                    value,
                    levels,
                })
            };
            let first = perlin()?;
            let second = perlin()?;
            normals.push(Normal {
                value_factor,
                first,
                second,
            });
        }
        let mut blended = Vec::new();
        for _ in 0..r.len(64)? {
            let mut ids = [0; 40];
            for id in &mut ids {
                *id = octave(r.u32()?)?;
            }
            blended.push(Blended {
                octaves: ids,
                scales: [r.f64()?, r.f64()?, r.f64()?, r.f64()?, r.f64()?],
            });
        }
        let flat_len = r.u32()?;
        let surface_root = r.u32()?;
        let surface = decode_code(&mut r, normals.len(), blended.len())?;
        let mut climate_roots = [0; 6];
        for root in &mut climate_roots {
            *root = r.u32()?;
        }
        let climate = decode_code(&mut r, normals.len(), blended.len())?;
        if flat_len as usize > surface.insts.len()
            || surface_root as usize >= surface.insts.len()
            || climate_roots
                .iter()
                .any(|root| *root as usize >= climate.insts.len())
        {
            return Err("far terrain program root out of range".into());
        }
        let mut biomes = Vec::new();
        for _ in 0..r.u16()? {
            let length = usize::from(r.u16()?);
            let name = std::str::from_utf8(r.0.take(length)?)
                .map_err(|_| "far terrain biome name is not UTF-8")?;
            biomes.push(name.to_owned());
        }
        let mut values = Vec::new();
        for _ in 0..r.u16()? {
            values.push(i64::from_le_bytes(r.bytes()?));
        }
        let mut rows = Vec::new();
        for _ in 0..r.len(1 << 16)? {
            let indices = r.bytes::<14>()?;
            let biome = r.u16()?;
            if usize::from(biome) >= biomes.len() {
                return Err("far terrain biome out of range".into());
            }
            let mut bounds = [[0; 2]; 7];
            for (bound, index) in bounds.iter_mut().flatten().zip(indices) {
                *bound = *values
                    .get(usize::from(index))
                    .ok_or("far terrain parameter out of range")?;
            }
            rows.push((bounds, biome));
        }
        r.0.done()?;
        if rows.is_empty() {
            return Err("far terrain program has no biomes".into());
        }
        Ok(Self {
            min_y,
            height,
            sea_level,
            cell_width,
            cell_height,
            octaves,
            normals,
            blended,
            surface,
            flat_len,
            surface_root,
            climate,
            climate_roots,
            biomes,
            rows,
        })
    }

    /// Registers one column evaluation needs.
    pub(crate) fn registers(&self) -> usize {
        self.surface.insts.len().max(self.climate.insts.len())
    }

    /// The server's `lod_column`: cave-free surface height and biome.
    pub(crate) fn column(&self, x: i32, z: i32, regs: &mut Vec<f64>) -> Column {
        regs.resize(self.registers(), 0.);
        let (width, height) = (self.cell_width, self.cell_height);
        let (cx, cz) = (x.div_euclid(width) * width, z.div_euclid(width) * width);
        let flat = self.flat_len as usize;
        let flat_position = [cx.div_euclid(4) * 4, 0, cz.div_euclid(4) * 4];
        self.run(&self.surface, 0..flat, flat_position, regs);
        let (min_y, max_y) = (self.min_y, self.min_y + self.height);
        let root = self.surface_root as usize;
        let mut upper = None;
        let mut y = ((max_y - 1).div_euclid(height) + 1) * height;
        let mut top = min_y - 1;
        while y >= min_y.div_euclid(height) * height {
            self.run(
                &self.surface,
                flat..self.surface.insts.len(),
                [cx, y, cz],
                regs,
            );
            let value = regs[root];
            if value > 0. {
                top = match upper {
                    None => max_y - 1,
                    Some(upper) => {
                        let dy = (0..height)
                            .rev()
                            .find(|dy| {
                                value + f64::from(*dy) / f64::from(height) * (upper - value) > 0.
                            })
                            .unwrap_or(0);
                        (y + dy).min(max_y - 1)
                    }
                };
                break;
            }
            upper = Some(value);
            y -= height;
        }
        let top = top.clamp(min_y, max_y - 1);
        let quart = [x, top, z].map(|v| v.div_euclid(4).wrapping_mul(4));
        self.run(&self.climate, 0..self.climate.insts.len(), quart, regs);
        let climate = self.climate_roots.map(|root| regs[root as usize]);
        Column {
            height: top,
            biome: self.biome(climate),
        }
    }

    /// The first parameter row nearest to the quantized climate.
    pub(crate) fn biome(&self, climate: [f64; 6]) -> u16 {
        let mut target = [0i64; 7];
        for (target, value) in target.iter_mut().zip(climate) {
            *target = ((value as f32) * 10000.0f32) as i64;
        }
        let mut best = (i64::MAX, 0);
        for (bounds, biome) in &self.rows {
            let mut distance = 0i64;
            for (bound, t) in bounds.iter().zip(target) {
                // Saturating: identical to the server's arithmetic for any
                // real parameter list, and no overflow on a hostile one.
                let d = t
                    .saturating_sub(bound[1])
                    .max(bound[0].saturating_sub(t))
                    .max(0);
                distance = distance.saturating_add(d.saturating_mul(d));
            }
            if distance < best.0 {
                best = (distance, *biome);
            }
        }
        best.1
    }

    fn run(&self, code: &Code, range: std::ops::Range<usize>, p: [i32; 3], regs: &mut [f64]) {
        let [px, py, pz] = p;
        for index in range {
            let Inst {
                op,
                args: [a, b, c, d],
                constants: [c0, c1],
            } = code.insts[index];
            let r = |k: u32| regs[k as usize];
            regs[index] = match op {
                0 => c0,
                1 => r(a) + r(b),
                2 => {
                    let a = r(a);
                    if a == 0. {
                        0.
                    } else {
                        a * r(b)
                    }
                }
                3 => r(a).min(r(b)),
                4 => r(a).max(r(b)),
                5 => r(a).abs(),
                6 => r(a) * r(a),
                7 => r(a) * r(a) * r(a),
                8 => {
                    let v = r(a);
                    if v > 0. {
                        v
                    } else {
                        v * 0.5
                    }
                }
                9 => {
                    let v = r(a);
                    if v > 0. {
                        v
                    } else {
                        v * 0.25
                    }
                }
                10 => 1. / r(a),
                11 => {
                    let v = r(a).clamp(-1., 1.);
                    v / 2. - v * v * v / 24.
                }
                12 => r(a).clamp(c0, c1),
                13 => {
                    let (from_y, to_y) = (a as i32, b as i32);
                    let t = ((f64::from(py) - f64::from(from_y)) / f64::from(to_y - from_y))
                        .clamp(0., 1.);
                    c0 + t * (c1 - c0)
                }
                14 => self.normal(
                    a,
                    f64::from(px) * c0,
                    f64::from(py) * c1,
                    f64::from(pz) * c0,
                ),
                15 => {
                    let (x, y, z) = match b {
                        0 => (px, py, pz),
                        1 => (px, 0, pz),
                        _ => (pz, px, 0),
                    };
                    self.normal(
                        a,
                        f64::from(x) * 0.25,
                        f64::from(y) * 0.25,
                        f64::from(z) * 0.25,
                    ) * 4.
                }
                16 => self.normal(
                    a,
                    f64::from(px) * c0 + r(b),
                    f64::from(py) * c1 + r(c),
                    f64::from(pz) * c0 + r(d),
                ),
                17 => {
                    let v = r(b);
                    let rarity = if c != 0 {
                        if v < -0.75 {
                            0.5
                        } else if v < -0.5 {
                            0.75
                        } else if v < 0.5 {
                            1.
                        } else if v < 0.75 {
                            2.
                        } else {
                            3.
                        }
                    } else if v < -0.5 {
                        0.75
                    } else if v < 0. {
                        1.
                    } else if v < 0.5 {
                        1.5
                    } else {
                        2.
                    };
                    rarity
                        * self
                            .normal(
                                a,
                                f64::from(px) / rarity,
                                f64::from(py) / rarity,
                                f64::from(pz) / rarity,
                            )
                            .abs()
                }
                18 => {
                    let v = r(a);
                    if v >= c0 && v < c1 {
                        r(b)
                    } else {
                        r(c)
                    }
                }
                19 => f64::from(spline(code, a, b, c, regs)),
                20 => self.blended(a, px, py, pz),
                _ => unreachable!("validated op"),
            };
        }
    }

    fn normal(&self, id: u32, x: f64, y: f64, z: f64) -> f64 {
        let noise = &self.normals[id as usize];
        let factor = 1.0181268882175227;
        (self.perlin(&noise.first, x, y, z)
            + self.perlin(&noise.second, x * factor, y * factor, z * factor))
            * noise.value_factor
    }

    fn perlin(&self, perlin: &Perlin, x: f64, y: f64, z: f64) -> f64 {
        let mut input = perlin.input;
        let mut weight = perlin.value;
        let mut sum = 0.;
        for (amplitude, octave) in &perlin.levels {
            if let Some(octave) = octave {
                sum += amplitude
                    * self.octaves[*octave as usize].sample(
                        wrap(x * input),
                        wrap(y * input),
                        wrap(z * input),
                        0.,
                        0.,
                    )
                    * weight;
            }
            input *= 2.;
            weight /= 2.;
        }
        sum
    }

    fn blended(&self, id: u32, x: i32, y: i32, z: i32) -> f64 {
        let noise = &self.blended[id as usize];
        let [xz, ys, xz_factor, y_factor, smear] = noise.scales;
        let octave = |index: usize| &self.octaves[noise.octaves[index] as usize];
        let x = f64::from(x) * xz;
        let y = f64::from(y) * ys;
        let z = f64::from(z) * xz;
        let mx = x / xz_factor;
        let my = y / y_factor;
        let mz = z / xz_factor;
        let smear = ys * smear;
        let main_smear = smear / y_factor;
        let mut main = 0.;
        let mut scale = 1.;
        for index in 32..40 {
            main += octave(index).sample(
                blended_wrap(mx * scale),
                blended_wrap(my * scale),
                blended_wrap(mz * scale),
                main_smear * scale,
                my * scale,
            ) / scale;
            scale /= 2.;
        }
        let alpha = (main / 10. + 1.) / 2.;
        let mut min = 0.;
        let mut max = 0.;
        scale = 1.;
        for index in 0..16 {
            let sx = blended_wrap(x * scale);
            let sy = blended_wrap(y * scale);
            let sz = blended_wrap(z * scale);
            if alpha < 1. {
                min += octave(index).sample(sx, sy, sz, smear * scale, y * scale) / scale;
            }
            if alpha > 0. {
                max += octave(16 + index).sample(sx, sy, sz, smear * scale, y * scale) / scale;
            }
            scale /= 2.;
        }
        let alpha = alpha.clamp(0., 1.);
        (min / 512. + alpha * (max / 512. - min / 512.)) / 128.
    }
}

fn decode_code(r: &mut Le<'_>, normals: usize, blended: usize) -> Result<Code> {
    let count = r.len(MAX_REGISTERS)?;
    let mut insts = Vec::with_capacity(count);
    for _ in 0..count {
        insts.push(Inst {
            op: r.u8()?,
            args: [r.u32()?, r.u32()?, r.u32()?, r.u32()?],
            constants: [r.f64()?, r.f64()?],
        });
    }
    let mut points = Vec::new();
    for _ in 0..r.len(1 << 16)? {
        let location = r.f32()?;
        let derivative = r.f32()?;
        let register = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err("invalid far terrain spline point".into()),
        };
        points.push(Point {
            location,
            derivative,
            register,
            value: r.u32()?,
        });
    }
    let invalid = |what: &str| Err(format!("invalid far terrain {what}"));
    for (index, inst) in insts.iter().enumerate() {
        let Some(registers) = REGISTER_ARGS.get(usize::from(inst.op)) else {
            return invalid("operation");
        };
        // Straight-line code: an instruction reads only earlier registers.
        for (bit, arg) in inst.args.iter().enumerate() {
            if registers & (1 << bit) != 0 && *arg as usize >= index {
                return invalid("register");
            }
        }
        let [a, b, c, _] = inst.args;
        let ok = match inst.op {
            12 => inst.constants[0] <= inst.constants[1],
            13 => a != b && [a, b].iter().all(|y| (*y as i32).unsigned_abs() < 1 << 20),
            14..=17 => (a as usize) < normals,
            19 => {
                let range = b as usize..b as usize + c as usize;
                c > 0
                    && range.end <= points.len()
                    && points[range.clone()]
                        .windows(2)
                        .all(|pair| pair[0].location < pair[1].location)
                    && points[range]
                        .iter()
                        .all(|point| !point.register || (point.value as usize) < index)
            }
            20 => (a as usize) < blended,
            _ => true,
        };
        if !ok {
            return invalid("instruction");
        }
    }
    Ok(Code { insts, points })
}

/// Vanilla's f32 spline step (`spline` in the server's `evaluate.rs`).
fn spline(code: &Code, coordinate: u32, first: u32, count: u32, regs: &[f64]) -> f32 {
    let points = &code.points[first as usize..(first + count) as usize];
    let input = regs[coordinate as usize] as f32;
    let value = |point: &Point| {
        if point.register {
            regs[point.value as usize] as f32
        } else {
            f32::from_bits(point.value)
        }
    };
    let next = points.partition_point(|point| point.location <= input);
    if next == 0 || next == points.len() {
        let edge = &points[if next == 0 { 0 } else { points.len() - 1 }];
        let v = value(edge);
        if edge.derivative == 0. {
            v
        } else {
            v + edge.derivative * (input - edge.location)
        }
    } else {
        let a = &points[next - 1];
        let b = &points[next];
        let width = b.location - a.location;
        let t = (input - a.location) / width;
        let y1 = value(a);
        let y2 = value(b);
        let da = a.derivative * width - (y2 - y1);
        let db = -b.derivative * width + (y2 - y1);
        y1 + t * (y2 - y1) + t * (1. - t) * (da + t * (db - da))
    }
}

impl Octave {
    /// Vanilla `ImprovedNoise.noise` with its Y smear.
    fn sample(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let p = |n: i32| i32::from(self.p[(n & 255) as usize]);
        let x = x + self.offset[0];
        let y = y + self.offset[1];
        let z = z + self.offset[2];
        let ix = floor_i32(x);
        let iy = floor_i32(y);
        let iz = floor_i32(z);
        let xr = x - f64::from(ix);
        let yr = y - f64::from(iy);
        let zr = z - f64::from(iz);
        let delta = if y_scale != 0. {
            let limited = if y_max >= 0. && y_max < yr { y_max } else { yr };
            (limited / y_scale + 1.0000000116860974e-7).floor() * y_scale
        } else {
            0.
        };
        let gy = yr - delta;
        let x0 = p(ix);
        let x1 = p(ix + 1);
        let a = p(x0 + iy);
        let b = p(x0 + iy + 1);
        let c = p(x1 + iy);
        let d = p(x1 + iy + 1);
        let fx = fade(xr);
        let fy = fade(yr);
        let fz = fade(zr);
        let lower = lerp(
            fy,
            lerp(
                fx,
                grad(p(a + iz), xr, gy, zr),
                grad(p(c + iz), xr - 1., gy, zr),
            ),
            lerp(
                fx,
                grad(p(b + iz), xr, gy - 1., zr),
                grad(p(d + iz), xr - 1., gy - 1., zr),
            ),
        );
        let upper = lerp(
            fy,
            lerp(
                fx,
                grad(p(a + iz + 1), xr, gy, zr - 1.),
                grad(p(c + iz + 1), xr - 1., gy, zr - 1.),
            ),
            lerp(
                fx,
                grad(p(b + iz + 1), xr, gy - 1., zr - 1.),
                grad(p(d + iz + 1), xr - 1., gy - 1., zr - 1.),
            ),
        );
        lerp(fz, lower, upper)
    }
}

/// `PerlinNoise.wrap`.
fn wrap(v: f64) -> f64 {
    let rounded = v / 33554432. + 0.5;
    if (0.0..1.0).contains(&rounded) {
        v
    } else {
        v - rounded.floor() * 33554432.
    }
}

/// The blended noise's own wrap.
fn blended_wrap(value: f64) -> f64 {
    value - (value / 33_554_432. + 0.5).floor() * 33_554_432.
}

/// Exactly `v.floor() as i32`, including saturation and NaN -> 0.
fn floor_i32(v: f64) -> i32 {
    let truncated = v as i32;
    truncated.saturating_sub(i32::from(v < f64::from(truncated)))
}

fn fade(v: f64) -> f64 {
    v * v * v * (v * (v * 6. - 15.) + 10.)
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

pub(crate) const GRADIENTS: [[f64; 3]; 16] = [
    [1., 1., 0.],
    [-1., 1., 0.],
    [1., -1., 0.],
    [-1., -1., 0.],
    [1., 0., 1.],
    [-1., 0., 1.],
    [1., 0., -1.],
    [-1., 0., -1.],
    [0., 1., 1.],
    [0., -1., 1.],
    [0., 1., -1.],
    [0., -1., -1.],
    [1., 1., 0.],
    [0., -1., 1.],
    [-1., 1., 0.],
    [0., -1., -1.],
];

fn grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    let g = GRADIENTS[(hash & 15) as usize];
    g[0] * x + g[1] * y + g[2] * z
}

#[cfg(test)]
#[path = "program_tests.rs"]
pub(crate) mod tests;
