//! Sheep coat layer: dyed wool over the base body, or the dyed undercoat once sheared.
//! The coat is Solaris-authored geometry (`sheep_wool`, `sheep_wool_baby`); the textures are the
//! player's vanilla assets. Wool colour and shear state come from synchronized `DATA_WOOL_ID`.
use super::{Entity, Models, presentation as p, resources::Rig};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content},
};
use glam::Mat4;

/// Dye diffuse colours indexed by dye id (white .. black).
const DYE_RGB: [u32; 16] = [
    0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97,
    0x169C9C, 0x8932B8, 0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
];

pub(super) fn dye_color(id: i32) -> [f32; 3] {
    let rgb = DYE_RGB[id.rem_euclid(16) as usize];
    [16, 8, 0].map(|shift| ((rgb >> shift) & 0xff) as f32 / 255.)
}

/// Draws the coat after the base body. `transforms` must hold the base rig's evaluated
/// bone matrices; they are replaced when the wool rig has to be re-evaluated.
pub(super) fn append(
    entity: &Entity,
    models: &Models,
    content: &Content,
    baby: bool,
    base_rig: &Rig,
    tick: u64,
    partial: f32,
    root: Mat4,
    transforms: &mut Vec<Mat4>,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    if entity.type_name != "minecraft:sheep" {
        return Ok(());
    }
    let flags = p::number(models, entity, "DATA_WOOL_ID").unwrap_or(0);
    let color = dye_color(flags & 15);
    if flags & 16 != 0 {
        // Sheared: the undercoat follows the base body exactly.
        if baby {
            return Ok(());
        }
        let Ok(texture) = p::texture(content, "minecraft:entity/sheep/sheep_wool_undercoat") else {
            return Ok(());
        };
        return p::emit_rig(
            base_rig,
            transforms,
            texture,
            color,
            EntityMaterial::EntityCutout,
            1.,
            light,
            out,
        );
    }
    let (key, texture) = if baby {
        (
            "minecraft:sheep_wool_baby#main",
            "minecraft:entity/sheep/sheep_wool_baby",
        )
    } else {
        (
            "minecraft:sheep_wool#main",
            "minecraft:entity/sheep/sheep_wool",
        )
    };
    if !models.has_authored_rig(key) {
        return Ok(());
    }
    let Ok(texture) = p::texture(content, texture) else {
        return Ok(());
    };
    let rig = models.rig(key);
    p::evaluate_rig(rig, entity, models, tick, partial, root, transforms);
    p::emit_rig(
        rig,
        transforms,
        texture,
        color,
        EntityMaterial::EntityCutout,
        1.,
        light,
        out,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dye_colors_decode_channels() {
        assert_eq!(dye_color(0), [249. / 255., 1., 254. / 255.]);
        assert_eq!(dye_color(14)[0], 0xB0 as f32 / 255.);
        assert_eq!(dye_color(16), dye_color(0));
    }
}
