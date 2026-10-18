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
    let transform = e.presented_transform(tick, partial);
    let direction = e.spawn_data;
    let rotation: f32 = match direction {
        2 => 180.,
        3 => 0.,
        4 => 90.,
        5 => 270.,
        _ => return Err("painting direction invalid".into()),
    };
    let root =
        translation(transform.position, origin) * Mat4::from_rotation_y(rotation.to_radians());
    let mut lights = [[0.; 2]; 256];
    // Resolve every segment before emitting: unobserved light cannot publish a half-painting.
    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - width as f64 * 0.5 + 0.5;
            let dy = y as f64 - height as f64 * 0.5 + 0.5;
            let mut point = transform.position.map(f64::floor);
            point[1] = (transform.position[1] + dy).floor();
            match direction {
                2 => point[0] = (transform.position[0] + dx).floor(),
                3 => point[0] = (transform.position[0] - dx).floor(),
                4 => point[2] = (transform.position[2] - dx).floor(),
                5 => point[2] = (transform.position[2] + dx).floor(),
                _ => unreachable!(),
            }
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
