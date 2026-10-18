//! Pinned PlaceRecipeHelper centers only dimensions smaller than half the crafting grid.
use crate::protocol::Result;
pub(super) fn shaped_slots(grid: usize, width: usize, height: usize) -> Result<Vec<usize>> {
    if !(2..=3).contains(&grid) || width == 0 || height == 0 || width > grid || height > grid {
        return Err("recipe_grid_dimensions".into());
    }
    let offset = |size| {
        if size * 2 < grid {
            (grid - size) / 2
        } else {
            0
        }
    };
    let x = offset(width);
    let y = offset(height);
    Ok((0..height)
        .flat_map(|row| (0..width).map(move |column| (row + y) * grid + column + x + 1))
        .collect())
}
