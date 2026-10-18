use super::*;

pub(super) fn evaluate(
    rig: &resources::Rig,
    root: Mat4,
    transforms: &mut Vec<Mat4>,
    mut animate: impl FnMut(&resources::Bone, &mut Vec3, &mut Vec3, &mut Vec3),
) {
    transforms.clear();
    transforms.reserve(rig.bones.len());
    for bone in &rig.bones {
        let mut pivot = bone.pivot;
        let mut rotation = bone.rotation;
        let mut scale = bone.scale;
        animate(bone, &mut pivot, &mut rotation, &mut scale);
        let parent = bone.parent.map(|index| transforms[index]).unwrap_or(root);
        transforms.push(parent * resources::transform(pivot, rotation, scale));
    }
}
pub(super) fn append_arrow(
    entity: &Entity,
    models: &Models,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light: [f32; 2],
    state: &State,
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    let family = models
        .families
        .get(&entity.type_name)
        .ok_or("projectile Source model family missing")?;
    let rig = models
        .rigs
        .get(&family.rig)
        .ok_or("projectile Source baked rig missing")?;
    let is_trident = family.renderer_class.as_deref()
        == Some("net.minecraft.client.renderer.entity.ThrownTridentRenderer");
    let transform = entity.presented_transform(tick, partial);
    let root = translation(transform.position, origin)
        * Mat4::from_rotation_y((transform.yaw - 90.).to_radians())
        * Mat4::from_rotation_z((transform.pitch + if is_trident { 90. } else { 0. }).to_radians());
    let shake = state.shake as f32 - partial;
    evaluate(rig, root, transforms, |bone, _, rotation, _| {
        if !is_trident && bone.parent.is_none() && shake > 0. {
            rotation.z += (-super::source_sin(shake * 3.) * shake).to_radians();
        }
    });
    let texture = if entity.type_name == "minecraft:arrow"
        && p::number(models, entity, "ID_EFFECT_COLOR").unwrap_or(-1) > 0
    {
        family
            .materials
            .get("TIPPED_ARROW_LOCATION")
            .and_then(Value::as_str)
    } else {
        family.texture.as_deref()
    }
    .ok_or("Source projectile material missing")?;
    p::emit_rig(
        rig,
        transforms,
        p::texture(content, texture)?,
        [1.; 3],
        EntityMaterial::EntityCutout,
        1.,
        light,
        out,
    )?;
    if is_trident && p::boolean(models, entity, "ID_FOIL") {
        p::emit_rig(
            rig,
            transforms,
            p::texture(content, "misc/enchanted_glint_entity")?,
            [1.; 3],
            EntityMaterial::EntityGlint,
            1.,
            [-1.; 2],
            out,
        )?;
    }
    Ok(Handled::Rendered)
}
