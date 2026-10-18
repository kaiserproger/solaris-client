use super::*;
use crate::ui::render::{TextAlignment, WorldTextOptions};
pub(super) struct Frame<'a> {
    pub root: Mat4,
    pub camera: [f64; 3],
    pub light: [f32; 2],
    pub text: Option<&'a TextRenderer>,
    pub cache: &'a mut Cache,
    pub scoping: bool,
    pub text_filtering: bool,
}
pub(super) fn append(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut Frame,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let hanging = e.type_name == "minecraft:hanging_sign";
    let name = block_name(e, c)?.trim_start_matches("minecraft:");
    let wall = name.contains("_wall_");
    let wood = if hanging {
        name.trim_end_matches("_hanging_sign")
            .trim_end_matches("_wall")
    } else {
        name.trim_end_matches("_sign").trim_end_matches("_wall")
    };
    let angle = if wall {
        yaw(prop(e, c, "facing").unwrap_or("north"))
    } else {
        prop(e, c, "rotation")
            .unwrap_or("0")
            .parse::<f32>()
            .map_err(|_| "sign rotation")?
            * 22.5
    };
    let attachment = if wall {
        "wall"
    } else if hanging && prop(e, c, "attached") == Some("true") {
        "ceiling_middle"
    } else if hanging {
        "ceiling"
    } else {
        "standing"
    };
    let key = if hanging {
        format!("minecraft:hanging_sign/{wood}/{attachment}#main")
    } else {
        format!("minecraft:sign/{attachment}/{wood}#main")
    };
    let tex = texture(
        c,
        &if hanging {
            format!("entity/signs/hanging/{wood}")
        } else {
            format!("entity/signs/{wood}")
        },
    )?;
    let mut plane = f.root
        * Mat4::from_translation(Vec3::new(0.5, if hanging { 0.625 } else { 0.5 }, 0.5))
        * Mat4::from_rotation_y(-angle.to_radians());
    if wall && !hanging {
        plane *= Mat4::from_translation(Vec3::new(0., -0.3125, -0.4375));
    }
    let scale = if hanging { 1. } else { 2. / 3. };
    paint_rig(
        rig(m, &key)?,
        plane * Mat4::from_scale(Vec3::new(scale, -scale, -scale)),
        tex,
        [1.; 3],
        EntityMaterial::EntityCutout,
        f.light,
        t,
        out,
        |_, _, _| {},
    )?;
    let Some(text) = f.text else {
        return Err("source sign font painter not activated".into());
    };
    let layouts = f.cache.sign(e, text, f.text_filtering, hanging)?;
    let line_height = if hanging { 9. } else { 10. };
    let offset = if hanging {
        Vec3::new(0., -0.32, 0.073)
    } else {
        Vec3::new(0., 1. / 3., 0.046666667)
    };
    let text_scale = if hanging { 0.0140625 } else { 0.010416667 };
    let center = e.position.map(|v| f64::from(v) + 0.5);
    let distance = (0..3)
        .map(|i| (center[i] - f.camera[i]).powi(2))
        .sum::<f64>();
    for (side, back) in [("front_text", false), ("back_text", true)] {
        let data = &e.data_value()[side];
        if data.is_null() {
            continue;
        }
        let glow = data["has_glowing_text"]
            .as_bool()
            .unwrap_or_else(|| data["has_glowing_text"].as_i64() == Some(1));
        let color = dye(data["color"].as_str().unwrap_or("black")).1;
        let dark = if glow && color == 0 {
            0xfff0ebccu32
        } else {
            let scaled = rgb(color).map(|v| (v * 255. * 0.4) as u32);
            0xff000000 | scaled[0] << 16 | scaled[1] << 8 | scaled[2]
        };
        let options = WorldTextOptions {
            line_height,
            alignment: TextAlignment::Center,
            light: if glow { [15.; 2] } else { f.light },
            layer: EntityMaterial::TextPolygonOffset,
            outline: (glow && (color == 0 || distance < 256. || f.scoping)).then_some(dark),
            ..Default::default()
        };
        let face = plane
            * if back {
                Mat4::from_rotation_y(PI)
            } else {
                Mat4::IDENTITY
            }
            * Mat4::from_translation(offset)
            * Mat4::from_scale(Vec3::new(text_scale, -text_scale, text_scale));
        let lines = if back { &layouts.back } else { &layouts.front };
        for (line, layout) in lines.iter().enumerate() {
            let matrix = face
                * Mat4::from_translation(Vec3::new(
                    -layout.width / 2.,
                    line as f32 * line_height - line_height * 2.,
                    0.,
                ));
            text.append_world_layout(
                c,
                layout,
                matrix,
                if glow { 0xff000000 | color } else { dark },
                &options,
                out,
            )?;
        }
    }
    // Wax is an interaction fact; it does not select a different material or text style.
    Ok(())
}
