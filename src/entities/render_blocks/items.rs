use super::*;
fn empty(item: &Value) -> bool {
    item.is_null()
        || item["id"].as_str().is_none_or(|v| v == "minecraft:air")
        || item["count"].as_i64() == Some(0)
}
pub(super) fn append(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    root: Mat4,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    match e.type_name.as_str() {
        "minecraft:campfire" => {
            let facing = yaw(prop(e, c, "facing").unwrap_or("north")) as i32 / 90;
            if let Some(items) = e.data_value()["Items"].as_array() {
                for item in items {
                    let slot = item["Slot"].as_u64().unwrap_or(0);
                    if slot >= 4 {
                        return Err("campfire item slot bound".into());
                    }
                    if empty(item) {
                        continue;
                    }
                    let angle = -((slot as i32 + facing) % 4) as f32 * 90.;
                    let matrix = root
                        * Mat4::from_translation(Vec3::new(0.5, 0.44921875, 0.5))
                        * Mat4::from_rotation_y(angle.to_radians())
                        * Mat4::from_rotation_x(FRAC_PI_2)
                        * Mat4::from_translation(Vec3::new(-0.3125, -0.3125, 0.))
                        * Mat4::from_scale(Vec3::splat(0.375));
                    (f.item)(
                        item,
                        "fixed",
                        (crate::protocol::pack_pos(e.position) as i32).wrapping_add(slot as i32),
                        matrix,
                        f.light,
                        out,
                    )?;
                }
            }
        }
        "minecraft:brushable_block" => {
            let progress = prop(e, c, "dusted")
                .unwrap_or("0")
                .parse::<u8>()
                .map_err(|_| "brush dust progress")?;
            let Some(direction) = e.data_value()["hit_direction"].as_u64() else {
                return Ok(BlockRenderStatus::SourceHidden);
            };
            let item = &e.data_value()["item"];
            if progress == 0 || empty(item) {
                return Ok(BlockRenderStatus::SourceHidden);
            }
            if direction > 5 || progress > 3 {
                return Err("brush state bound".into());
            }
            let translation = math::brush_translation(direction as u8, progress);
            let direction_vector = super::super::physical::direction(direction as i32)?;
            let light_point =
                std::array::from_fn(|i| f64::from(e.position[i]) + direction_vector[i]);
            let Some(light) = (f.sample_light)(light_point)? else {
                return Ok(BlockRenderStatus::WorldPending);
            };
            let angle: f32 = 75.
                + if direction == 4 || direction == 5 {
                    101.
                } else {
                    11.
                };
            let matrix = root
                * Mat4::from_translation(Vec3::from_array(translation))
                * Mat4::from_rotation_y(angle.to_radians())
                * Mat4::from_scale(Vec3::splat(0.5));
            (f.item)(item, "fixed", 0, matrix, light, out)?;
        }
        "minecraft:shelf" => {
            let bottom = e.data_value()["align_items_to_bottom"]
                .as_bool()
                .unwrap_or_else(|| e.data_value()["align_items_to_bottom"].as_i64() == Some(1));
            if let Some(items) = e.data_value()["Items"].as_array() {
                for item in items {
                    let slot = item["Slot"].as_u64().unwrap_or(0);
                    if slot >= 3 {
                        return Err("shelf item slot bound".into());
                    }
                    if empty(item) {
                        continue;
                    }
                    let matrix = root
                        * Mat4::from_translation(Vec3::splat(0.5))
                        * Mat4::from_rotation_y(
                            -yaw(prop(e, c, "facing").unwrap_or("north")).to_radians(),
                        )
                        * Mat4::from_translation(Vec3::new(
                            (slot as f32 - 1.) * 0.3125,
                            if bottom { -0.25 } else { 0. },
                            -0.25,
                        ))
                        * Mat4::from_scale(Vec3::splat(0.25));
                    (f.item)(
                        item,
                        if bottom {
                            "on_shelf_bottom"
                        } else {
                            "on_shelf_center"
                        },
                        {
                            let packed = crate::protocol::pack_pos(e.position);
                            ((packed as i32) ^ ((packed >> 32) as i32)).wrapping_add(slot as i32)
                        },
                        matrix,
                        f.light,
                        out,
                    )?;
                }
            }
        }
        "minecraft:vault" => {
            let item = &e.data_value()["shared_data"]["display_item"];
            if empty(item) {
                return Ok(BlockRenderStatus::SourceHidden);
            }
            let s = &e.animation.source;
            let delta = (s.spin - s.previous_spin + 180.).rem_euclid(360.) - 180.;
            let matrix = root
                * Mat4::from_translation(Vec3::new(0.5, 0.4, 0.5))
                * Mat4::from_rotation_y((s.previous_spin + delta * f.partial).to_radians());
            (f.item)(item, "ground_cluster", 0, matrix, f.light, out)?;
        }
        "minecraft:mob_spawner" | "minecraft:trial_spawner" => {
            let trial = e.type_name == "minecraft:trial_spawner";
            if trial
                && !matches!(
                    prop(e, c, "trial_spawner_state"),
                    Some("waiting_for_players" | "active")
                )
            {
                return Ok(BlockRenderStatus::SourceHidden);
            }
            let entity = &e.data_value()[if trial { "spawn_data" } else { "SpawnData" }]["entity"];
            let Some(name) = entity["id"].as_str() else {
                return Ok(BlockRenderStatus::SourceHidden);
            };
            if !m.schemas.contains_key(name) {
                return Err(format!("unknown spawner display entity: {name}"));
            }
            let s = &e.animation.source;
            let spin = (s.previous_spawn_spin
                + (s.spawn_spin - s.previous_spawn_spin) * f64::from(f.partial))
                as f32
                * 10.;
            let matrix = root
                * Mat4::from_translation(Vec3::new(0.5, 0.4, 0.5))
                * Mat4::from_rotation_y(spin.to_radians())
                * Mat4::from_translation(Vec3::new(0., -0.2, 0.))
                * Mat4::from_rotation_x(-30f32.to_radians());
            // The family painter derives the preview's .53125/max(1,width,height)
            // scale from its actual NBT/metadata physical dimensions.
            let status = (f.entity)(entity, matrix, f.light, out)?;
            if status != BlockRenderStatus::Rendered {
                return Ok(status);
            }
        }
        _ => unreachable!(),
    }
    Ok(BlockRenderStatus::Rendered)
}
