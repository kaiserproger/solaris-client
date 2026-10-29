use super::*;

// 26.3 ItemFrameRenderer uses the packet block anchor, direction rotation and
// centered block-model transform; frame geometry comes from the user's pack.
fn frame_root(anchor: [f64; 3], origin: [f64; 3], direction: i32) -> Result<Mat4> {
    let (pitch, yaw) = match direction {
        0 => (std::f32::consts::FRAC_PI_2, std::f32::consts::PI),
        1 => (-std::f32::consts::FRAC_PI_2, std::f32::consts::PI),
        2 => (0., 0.),
        3 => (0., std::f32::consts::PI),
        4 => (0., std::f32::consts::FRAC_PI_2),
        5 => (0., -std::f32::consts::FRAC_PI_2),
        _ => return Err("item frame direction invalid".into()),
    };
    Ok(translation(anchor.map(|coordinate| coordinate.floor() + 0.5), origin)
        * Mat4::from_rotation_x(pitch) * Mat4::from_rotation_y(yaw))
}

#[expect(clippy::too_many_arguments, reason = "Matches the entity extraction inputs without copying render state")]
pub(super) fn append(
    entity: &Entity,
    models: &Models,
    cache: &mut Cache,
    items: &mut ItemModels,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let direction = match p::value(models, entity, "DATA_DIRECTION") {
        Some(value) => value.int().ok_or("item frame direction metadata invalid")?,
        None => entity.spawn_data,
    };
    let root = frame_root(entity.presented_transform(tick, partial).position, origin, direction)?;
    let stack = match p::value(models, entity, "DATA_ITEM") {
        Some(Metadata::Item(stack)) if !stack.empty() => Some(stack),
        _ => None,
    };
    let invisible = p::number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 32 != 0;
    let glow = entity.type_name == "minecraft:glow_item_frame";
    let item_model = stack.map(|stack| items.model(content, stack, "fixed")).transpose()?;
    if !invisible {
        let model_name = if glow { "minecraft:block/glow_item_frame" }
            else { "minecraft:block/item_frame" };
        let model = cache.model(content, model_name)?;
        let mut body_light = light;
        if glow { body_light[0] = body_light[0].max(5.); }
        p::append_baked(model, root * Mat4::from_translation(Vec3::splat(-0.5)), body_light, out)?;
    }
    if let Some(model) = item_model {
        let rotation = p::number(models, entity, "DATA_ROTATION").unwrap_or(0).rem_euclid(8);
        let item = root
            * Mat4::from_translation(Vec3::new(0., 0., if invisible { 0.5 } else { 0.4375 }))
            * Mat4::from_rotation_z(rotation as f32 * std::f32::consts::FRAC_PI_4)
            * Mat4::from_scale(Vec3::splat(0.5));
        p::append_baked(model, item * p::item_matrix(model, "fixed"), if glow { [15.;2] } else { light }, out)?;
    }
    Ok(if invisible && stack.is_none() { Handled::NoGeometry } else { Handled::Rendered })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_model_front_faces_outward_on_all_six_surfaces() {
        for (direction, outward) in [(0, Vec3::NEG_Y), (1, Vec3::Y),
            (2, Vec3::NEG_Z), (3, Vec3::Z), (4, Vec3::NEG_X), (5, Vec3::X)] {
            let root = frame_root([4., 5., 6.], [4., 5., 6.], direction).unwrap();
            assert!((root.transform_vector3(Vec3::NEG_Z) - outward).length() < 0.00001);
            assert!((root.transform_point3(Vec3::ZERO) - Vec3::splat(0.5)).length() < 0.00001);
        }
    }
}
