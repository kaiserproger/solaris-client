//! Per-section face connectivity for occlusion culling.
//!
//! Face order is DOWN UP NORTH SOUTH WEST EAST; opposite face is `face ^ 1`.
//! Bit `from * 6 + to` is set when an open (non-occluding) path exists through
//! the 16^3 section from face `from` to face `to`. Open sections set all 36
//! bits, fully opaque sections set none.

/// Open (missing/empty) sections connect every face pair.
pub(crate) const ALL_OPEN: u64 = (1u64 << 36) - 1;
/// Opaque means conservative occluder
/// (`fact.solid_render && fact.can_occlude && fact.collision_full`).
pub(super) fn build(opaque: &[bool; 4096]) -> u64 {
    let mut open_count = 0usize;
    for cell in opaque.iter() {
        if !*cell {
            open_count += 1;
        }
    }
    if open_count == 0 {
        return 0;
    }
    if open_count == 4096 {
        return ALL_OPEN;
    }
    let mut mask = 0u64;
    let mut visited = [false; 4096];
    let mut stack = [0usize; 4096];
    for start in 0..4096 {
        if opaque[start] || visited[start] {
            continue;
        }
        // Flood one open component, recording which boundary faces it touches.
        let mut faces: u8 = 0;
        let mut top = 0usize;
        stack[top] = start;
        top += 1;
        visited[start] = true;
        while top > 0 {
            top -= 1;
            let current = stack[top];
            let x = current % 16;
            let z = (current / 16) % 16;
            let y = current / 256;
            if y == 0 {
                faces |= 1 << 0;
            }
            if y == 15 {
                faces |= 1 << 1;
            }
            if z == 0 {
                faces |= 1 << 2;
            }
            if z == 15 {
                faces |= 1 << 3;
            }
            if x == 0 {
                faces |= 1 << 4;
            }
            if x == 15 {
                faces |= 1 << 5;
            }
            // Six-neighbor walk through open cells only.
            if x > 0 {
                let next = current - 1;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
            if x < 15 {
                let next = current + 1;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
            if z > 0 {
                let next = current - 16;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
            if z < 15 {
                let next = current + 16;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
            if y > 0 {
                let next = current - 256;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
            if y < 15 {
                let next = current + 256;
                if !opaque[next] && !visited[next] {
                    visited[next] = true;
                    stack[top] = next;
                    top += 1;
                }
            }
        }
        for from in 0..6 {
            if faces & (1 << from) == 0 {
                continue;
            }
            for to in 0..6 {
                if faces & (1 << to) != 0 {
                    mask |= 1u64 << (from * 6 + to);
                }
            }
        }
        if mask == ALL_OPEN {
            break;
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> [bool; 4096] {
        [false; 4096]
    }

    #[test]
    fn open_section_connects_every_face_pair() {
        assert_eq!(build(&open()), ALL_OPEN);
        assert_eq!(ALL_OPEN, (1u64 << 36) - 1);
    }

    #[test]
    fn solid_section_connects_nothing() {
        assert_eq!(build(&[true; 4096]), 0);
    }

    #[test]
    fn full_wall_blocks_west_east_but_keeps_vertical_path() {
        let mut opaque = [false; 4096];
        for y in 0..16 {
            for z in 0..16 {
                opaque[y * 256 + z * 16 + 8] = true;
            }
        }
        let mask = build(&opaque);
        // No open path crosses the x=8 plane.
        assert_eq!(mask & (1u64 << (4 * 6 + 5)), 0, "west must not reach east");
        assert_eq!(mask & (1u64 << (5 * 6 + 4)), 0, "east must not reach west");
        // Each side still touches its own face and runs vertically.
        assert_ne!(mask & (1u64 << (4 * 6 + 4)), 0);
        assert_ne!(mask & (1u64 << (5 * 6 + 5)), 0);
        assert_ne!(
            mask & (1u64 << 1),
            0,
            "down still reaches up around wall ends"
        );
    }

    #[test]
    fn wall_with_single_hole_restores_west_east_path() {
        let mut opaque = [false; 4096];
        for y in 0..16 {
            for z in 0..16 {
                opaque[y * 256 + z * 16 + 8] = true;
            }
        }
        // Tunnel through the wall.
        opaque[8 * 256 + 8 * 16 + 8] = false;
        let mask = build(&opaque);
        assert_ne!(
            mask & (1u64 << (4 * 6 + 5)),
            0,
            "tunnel must connect west to east"
        );
        assert_ne!(
            mask & (1u64 << (5 * 6 + 4)),
            0,
            "tunnel must connect east to west"
        );
    }

    #[test]
    fn sealed_floor_blocks_down_up_only() {
        let mut opaque = [false; 4096];
        for z in 0..16 {
            for x in 0..16 {
                opaque[8 * 256 + z * 16 + x] = true;
            }
        }
        let mask = build(&opaque);
        assert_eq!(mask & (1u64 << 1), 0);
        assert_eq!(mask & (1u64 << 6), 0);
        assert_ne!(
            mask & (1u64 << (2 * 6 + 3)),
            0,
            "north-south stays open above the floor"
        );
    }
}
