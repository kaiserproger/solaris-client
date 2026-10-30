//! Flat drifting cloud layer built from the game's own `environment/clouds`
//! texture at runtime. Its cell size, thickness and motion are authored visual
//! approximations; height comes from received environment attributes.
use crate::{
    api::{DynamicGeometry, EntityMaterial, SkyKind, Vertex, WorldEnvironment},
    protocol::{Result, content::Content},
};

const CELL: f64 = 11.8;
const THICKNESS: f64 = 3.8;
/// Blocks per game tick the layer drifts toward -X.
const DRIFT: f64 = 0.028;
/// Own visual coverage choice (~700 blocks), fitted to the h-far oracle's
/// near-horizon cloud band rather than the old ~236-block cutoff.
const RADIUS: i32 = 60;
const ALPHA: f32 = 0.8;
const MAX_VERTICES: usize = 1_900_000;
const TEXTURE: &str = "minecraft:environment/clouds";

/// Occupancy of the repeating cloud texture (row-major, `size` x `size`).
pub(super) struct Cells {
    size: usize,
    solid: Vec<bool>,
}

impl Cells {
    fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Self> {
        if width == 0 || width != height || rgba.len() != (width * height * 4) as usize {
            return Err("cloud texture must be square RGBA".into());
        }
        Ok(Self {
            size: width as usize,
            solid: rgba.chunks_exact(4).map(|p| p[3] > 127).collect(),
        })
    }
    fn at(&self, x: i64, z: i64) -> bool {
        let size = self.size as i64;
        self.solid[(z.rem_euclid(size) * size + x.rem_euclid(size)) as usize]
    }
}

#[derive(Default)]
pub(super) struct Clouds {
    cells: Option<(u64, Cells)>,
}

/// Cloud tint: white by day, dark at night (sun below the horizon), pulled
/// toward grey in rain.
pub(super) fn tint(sun_angle_deg: f32, rain: f32) -> [f32; 3] {
    let day = ((sun_angle_deg.to_radians().cos() * 2. + 0.5).clamp(0., 1.)) * 0.9 + 0.1;
    let rain = rain.clamp(0., 1.);
    let grey = day * 0.6;
    let keep = 1. - rain * 0.95;
    let channel = day * keep + grey * (1. - keep);
    [channel; 3]
}

/// Where the layer is seen from: resource epoch (invalidates the cached
/// texture), camera and scene origin in world blocks, and game ticks.
#[derive(Clone, Copy)]
pub(super) struct View {
    pub epoch: u64,
    pub camera: [f64; 3],
    pub origin: [f64; 3],
    pub ticks: f64,
}

impl Clouds {
    pub(super) fn append(
        &mut self,
        content: &Content,
        view: &View,
        env: &WorldEnvironment,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let View { epoch, camera, origin, ticks } = *view;
        // A resource stack without the texture simply has no cloud layer.
        if env.sky_kind != SkyKind::Overworld || content.texture(TEXTURE).is_none() {
            return Ok(());
        }
        if self.cells.as_ref().is_none_or(|(loaded, _)| *loaded != epoch) {
            let (width, height, rgba) = content.texture_pixels(TEXTURE)?;
            self.cells = Some((epoch, Cells::from_rgba(width, height, &rgba)?));
        }
        let cells = &self.cells.as_ref().expect("cloud cells loaded").1;
        let white = content
            .texture("solaris:white")
            .ok_or("missing mandatory texture solaris:white")?;
        let uv = [(white.uv[0] + white.uv[2]) / 2., (white.uv[1] + white.uv[3]) / 2.];
        let tint = tint(env.sun_angle, env.rain);
        let scroll = ticks * DRIFT;
        let (base_x, base_z) = (
            ((camera[0] + scroll) / CELL).floor() as i64,
            (camera[2] / CELL).floor() as i64,
        );
        let (y0, y1) = (f64::from(env.cloud_height) - origin[1], f64::from(env.cloud_height) + THICKNESS - origin[1]);
        let emit = |positions: [[f64; 3]; 4], shade: f32, out: &mut DynamicGeometry| -> Result<()> {
            if out.vertices.len() + 4 > MAX_VERTICES {
                return Err("cloud geometry vertex budget reached".into());
            }
            let first = u32::try_from(out.vertices.len()).map_err(|_| "cloud index overflow")?;
            let normal = {
                let a = std::array::from_fn::<f64, 3, _>(|i| positions[1][i] - positions[0][i]);
                let b = std::array::from_fn::<f64, 3, _>(|i| positions[2][i] - positions[0][i]);
                let n = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                n.map(|v| (v / length) as f32)
            };
            for position in positions {
                out.vertices.push(Vertex {
                    position: position.map(|v| v as f32),
                    color: tint.map(|c| c * shade),
                    uv,
                    overlay_uv: [0., 10.],
                    overlay_tint: [1.; 3],
                    alpha: ALPHA,
                    texture_layer: white.layer,
                    light: [15., 0.],
                    normal,
                });
            }
            let indices = [first, first + 1, first + 2, first + 2, first + 3, first];
            out.indices[EntityMaterial::CloudsDepth as usize].extend(indices);
            out.indices[EntityMaterial::Clouds as usize].extend(indices);
            Ok(())
        };
        let world_x = |i: i64| i as f64 * CELL - scroll - origin[0];
        let world_z = |j: i64| j as f64 * CELL - origin[2];
        for dz in -RADIUS..=RADIUS {
            let j = base_z + i64::from(dz);
            let span = (f64::from(RADIUS * RADIUS - dz * dz)).sqrt() as i64;
            let (first, last) = (base_x - span, base_x + span);
            let (z0, z1) = (world_z(j), world_z(j + 1));
            let mut i = first;
            while i <= last {
                if !cells.at(i, j) {
                    i += 1;
                    continue;
                }
                // One top and bottom quad per run of cloud cells along X.
                let start = i;
                while i < last && cells.at(i + 1, j) {
                    i += 1;
                }
                let (x0, x1) = (world_x(start), world_x(i + 1));
                emit([[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]], 1., out)?;
                emit([[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]], 0.7, out)?;
                emit([[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]], 0.9, out)?;
                emit([[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]], 0.9, out)?;
                for cell in start..=i {
                    let (cx0, cx1) = (world_x(cell), world_x(cell + 1));
                    if !cells.at(cell, j - 1) {
                        emit([[cx0, y0, z0], [cx0, y1, z0], [cx1, y1, z0], [cx1, y0, z0]], 0.8, out)?;
                    }
                    if !cells.at(cell, j + 1) {
                        emit([[cx0, y0, z1], [cx1, y0, z1], [cx1, y1, z1], [cx0, y1, z1]], 0.8, out)?;
                    }
                }
                i += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker() -> Cells {
        // 4x4 texture: a 2x1 cloud at (1..3, 1).
        let mut rgba = vec![0; 4 * 4 * 4];
        for x in 1..3 {
            rgba[(4 + x) * 4 + 3] = 255;
        }
        Cells::from_rgba(4, 4, &rgba).unwrap()
    }

    #[test]
    fn cells_follow_texture_alpha_and_wrap() {
        let cells = checker();
        assert!(cells.at(1, 1) && cells.at(2, 1));
        assert!(!cells.at(0, 1) && !cells.at(1, 0));
        assert!(cells.at(1 + 4, 1 - 4) && cells.at(-3, 1));
    }

    #[test]
    fn tint_is_white_by_day_dark_at_night_and_greyer_in_rain() {
        assert_eq!(tint(0., 0.), [1.; 3]);
        assert!(tint(180., 0.)[0] < 0.2);
        assert!(tint(0., 1.)[0] < tint(0., 0.)[0]);
    }

    #[test]
    fn cloud_geometry_reaches_the_oracle_horizon_band() {
        let mut content = Content::test_cube();
        let region = crate::protocol::content::TextureRegion {
            uv: [0., 0., 1., 1.], layer: 0, width: 1, height: 1,
        };
        content.insert_test_texture(TEXTURE, region);
        content.insert_test_texture("solaris:white", region);
        let mut clouds = Clouds {
            cells: Some((1, Cells { size: 1, solid: vec![true] })),
        };
        let mut geometry = DynamicGeometry::default();
        clouds.append(
            &content,
            &View { epoch: 1, camera: [0.; 3], origin: [0.; 3], ticks: 0. },
            &WorldEnvironment { sky_kind: SkyKind::Overworld, cloud_height: 100., ..Default::default() },
            &mut geometry,
        ).unwrap();
        // Own coverage threshold: the h-far oracle has clouds below the band
        // reached by a roughly 236-block radius, well beyond 500 blocks.
        assert!(geometry.vertices.iter().any(|vertex| vertex.position[0] > 500.));
    }

    #[test]
    fn non_square_texture_is_rejected() {
        assert!(Cells::from_rgba(2, 1, &[0; 8]).is_err());
    }
}
