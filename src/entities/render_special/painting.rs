use super::*;

pub(super) fn append(
    e: &Entity,
    store: &Store,
    models: &Models,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light_source: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let (name, variant) = if let Some(Metadata::InlinePaintingVariant(variant)) =
        p::value(models, e, "DATA_PAINTING_VARIANT_ID")
    {
        (variant["asset_id"].as_str().ok_or("painting asset unavailable")?, variant)
    } else {
        let id = p::number(models, e, "DATA_PAINTING_VARIANT_ID").unwrap_or(0);
        let registry = "minecraft:painting_variant";
        let (name, observed) = store
            .registries
            .get(registry)
            .and_then(|entries| entries.get(id as usize))
            .ok_or("painting variant registry unavailable")?;
        let variant = if observed.is_null() {
            models
                .builtin_variants
                .get(registry)
                .and_then(|entries| entries.get(name))
                .ok_or("painting source variant unavailable")?
        } else {
            observed
        };
        (name.as_str(), variant)
    };
    let width = variant["width"]
        .as_u64()
        .ok_or("painting width unavailable")? as usize;
    let height = variant["height"]
        .as_u64()
        .ok_or("painting height unavailable")? as usize;
    if width == 0 || height == 0 || width > 16 || height > 16 {
        return Err("painting source dimensions bound".into());
    }
    let asset = variant["asset_id"].as_str().unwrap_or(name);
    let (namespace, path) = asset.split_once(':').unwrap_or(("minecraft", asset));
    let front = p::texture(content, &format!("{namespace}:painting/{path}"))?;
    let back = p::texture(content, "painting/back")?;
    let direction = match p::value(models, e, "DATA_DIRECTION") {
        Some(value) => value.int().ok_or("painting direction metadata invalid")?,
        None => e.spawn_data,
    };
    let anchor = e.presented_transform(tick, partial).position.map(f64::floor);
    let (center, rotation) = painting_placement(anchor, direction, width, height)?;
    let root = translation(center, origin) * Mat4::from_rotation_y(rotation);
    let mut lights = [[0.; 2]; 256];
    // Resolve every segment before emitting: unobserved light cannot publish a half-painting.
    for y in 0..height {
        for x in 0..width {
            let point = painting_light_position(center, direction, width, height, x, y);
            let Some(light) = p::sampled(light_source, point)? else {
                return Ok(Handled::PendingLighting);
            };
            lights[x + y * width] = light;
        }
    }
    for x in 0..width {
        for y in 0..height {
            let x1 = x as f32 - width as f32 * 0.5;
            let x0 = x1 + 1.;
            let y1 = y as f32 - height as f32 * 0.5;
            let y0 = y1 + 1.;
            let light = lights[x + y * width];
            let u0 = (width - x) as f32 / width as f32;
            let u1 = (width - x - 1) as f32 / width as f32;
            let v0 = (height - y) as f32 / height as f32;
            let v1 = (height - y - 1) as f32 / height as f32;
            painting_quad(
                root,
                [
                    [x0, y1, -0.03125],
                    [x1, y1, -0.03125],
                    [x1, y0, -0.03125],
                    [x0, y0, -0.03125],
                ],
                [[u1, v0], [u0, v0], [u0, v1], [u1, v1]],
                front,
                light,
                out,
            )?;
            painting_quad(
                root,
                [
                    [x0, y0, 0.03125],
                    [x1, y0, 0.03125],
                    [x1, y1, 0.03125],
                    [x0, y1, 0.03125],
                ],
                [[1., 0.], [0., 0.], [0., 1.], [1., 1.]],
                back,
                light,
                out,
            )?;
            if y + 1 == height {
                painting_quad(
                    root,
                    [
                        [x0, y0, -0.03125],
                        [x1, y0, -0.03125],
                        [x1, y0, 0.03125],
                        [x0, y0, 0.03125],
                    ],
                    [[0., 0.], [1., 0.], [1., 0.0625], [0., 0.0625]],
                    back,
                    light,
                    out,
                )?;
            }
            if y == 0 {
                painting_quad(
                    root,
                    [
                        [x0, y1, 0.03125],
                        [x1, y1, 0.03125],
                        [x1, y1, -0.03125],
                        [x0, y1, -0.03125],
                    ],
                    [[0., 0.], [1., 0.], [1., 0.0625], [0., 0.0625]],
                    back,
                    light,
                    out,
                )?;
            }
            if x + 1 == width {
                painting_quad(
                    root,
                    [
                        [x0, y0, 0.03125],
                        [x0, y1, 0.03125],
                        [x0, y1, -0.03125],
                        [x0, y0, -0.03125],
                    ],
                    [[0.0625, 0.], [0.0625, 1.], [0., 1.], [0., 0.]],
                    back,
                    light,
                    out,
                )?;
            }
            if x == 0 {
                painting_quad(
                    root,
                    [
                        [x1, y0, -0.03125],
                        [x1, y1, -0.03125],
                        [x1, y1, 0.03125],
                        [x1, y0, 0.03125],
                    ],
                    [[0.0625, 0.], [0.0625, 1.], [0., 1.], [0., 0.]],
                    back,
                    light,
                    out,
                )?;
            }
        }
    }
    Ok(Handled::Rendered)
}

// The add-entity position is the hanging block anchor, not the mesh center.
// Local -Z is the painted face; local +X follows the segment light order.
fn painting_placement(
    anchor: [f64; 3],
    direction: i32,
    width: usize,
    height: usize,
) -> Result<([f64; 3], f32)> {
    let (step, left, rotation) = match direction {
        2 => ([0., 0., -1.], [-1., 0., 0.], 0.),
        3 => ([0., 0., 1.], [1., 0., 0.], std::f32::consts::PI),
        4 => ([-1., 0., 0.], [0., 0., 1.], std::f32::consts::FRAC_PI_2),
        5 => ([1., 0., 0.], [0., 0., -1.], -std::f32::consts::FRAC_PI_2),
        _ => return Err("painting direction invalid".into()),
    };
    let width_offset = if width.is_multiple_of(2) { 0.5 } else { 0. };
    let height_offset = if height.is_multiple_of(2) { 0.5 } else { 0. };
    Ok((
        [
            anchor[0] + 0.5 - step[0] * 0.46875 + left[0] * width_offset,
            anchor[1] + 0.5 + height_offset,
            anchor[2] + 0.5 - step[2] * 0.46875 + left[2] * width_offset,
        ],
        rotation,
    ))
}

fn painting_light_position(
    center: [f64; 3],
    direction: i32,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> [f64; 3] {
    let dx = x as f64 - width as f64 * 0.5 + 0.5;
    let dy = y as f64 - height as f64 * 0.5 + 0.5;
    let mut point = center.map(f64::floor);
    point[1] = (center[1] + dy).floor();
    match direction {
        2 => point[0] = (center[0] + dx).floor(),
        3 => point[0] = (center[0] - dx).floor(),
        4 => point[2] = (center[2] - dx).floor(),
        5 => point[2] = (center[2] + dx).floor(),
        _ => unreachable!("painting placement validated direction"),
    }
    point
}
fn painting_quad(
    root: Mat4,
    points: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    tex: TextureRegion,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    textured_quad(
        root,
        points,
        uv,
        tex,
        [1.; 3],
        1.,
        EntityMaterial::EntitySolidZOffsetForward,
        light,
        out,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painting_anchor_orientation_and_segment_lighting() {
        let cases = [
            (2, [0., 0., -1.], [10., 21., 30.96875], [[9., 20., 30.], [10., 21., 30.]]),
            (3, [0., 0., 1.], [11., 21., 30.03125], [[11., 20., 30.], [10., 21., 30.]]),
            (4, [-1., 0., 0.], [10.96875, 21., 31.], [[10., 20., 31.], [10., 21., 30.]]),
            (5, [1., 0., 0.], [10.03125, 21., 30.], [[10., 20., 29.], [10., 21., 30.]]),
        ];
        for (direction, outward, expected_center, expected_light) in cases {
            let (center, rotation) =
                painting_placement([10., 20., 30.], direction, 2, 2).unwrap();
            assert_eq!(center, expected_center);
            assert_eq!(painting_light_position(center, direction, 2, 2, 0, 0), expected_light[0]);
            assert_eq!(painting_light_position(center, direction, 2, 2, 1, 1), expected_light[1]);
            let face = Mat4::from_rotation_y(rotation).transform_vector3(Vec3::NEG_Z);
            let expected = Vec3::from_array(outward);
            assert!((face - expected).length() < 0.00001);
            // The back of the 1/16-thick mesh lies exactly on the support face.
            let back = [
                center[0] - f64::from(outward[0]) * 0.03125,
                center[1],
                center[2] - f64::from(outward[2]) * 0.03125,
            ];
            let normal_axis = if outward[0] != 0. { 0 } else { 2 };
            let boundary = [10., 20., 30.][normal_axis]
                + if outward[normal_axis] < 0. { 1. } else { 0. };
            assert_eq!(back[normal_axis], boundary);
        }
    }

    #[test]
    fn painting_even_offsets_are_independent() {
        let anchor = [-4., 7., -9.];
        assert_eq!(painting_placement(anchor, 2, 1, 1).unwrap().0, [-3.5, 7.5, -8.03125]);
        assert_eq!(painting_placement(anchor, 2, 4, 3).unwrap().0, [-4., 7.5, -8.03125]);
        assert_eq!(painting_placement(anchor, 2, 3, 4).unwrap().0, [-3.5, 8., -8.03125]);
        assert_eq!(
            painting_light_position([-3.5, 8., -8.03125], 2, 3, 4, 0, 0),
            [-5., 6., -9.]
        );
        assert!(painting_placement(anchor, 1, 1, 1).is_err());
    }
}
