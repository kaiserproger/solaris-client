use crate::{
    inventory::{Menu, MenuKind},
    protocol::Result,
};
use serde::Serialize;
#[derive(Clone, Copy, Debug, Serialize)]
pub struct SlotRect {
    pub index: usize,
    pub x: f32,
    pub y: f32,
}
#[derive(Clone, Debug)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub inventory_label: [f32; 2],
    pub slots: Vec<SlotRect>,
}
impl Layout {
    pub fn menu(m: &Menu) -> Result<Self> {
        let mut out = Self {
            width: 176.,
            height: 166.,
            inventory_label: [8., 72.],
            slots: Vec::with_capacity(m.slots.len()),
        };
        let mut player = (8., 84.);
        let mut player_start;
        match m.kind {
            MenuKind::Inventory => {
                out.slot(0, 154., 28.);
                out.grid(1, 98., 18., 2, 2);
                for i in 0..4 {
                    out.slot(5 + i, 8., 8. + i as f32 * 18.);
                }
                out.standard(9, 8., 84.);
                out.slot(45, 77., 62.);
                return Ok(out);
            }
            MenuKind::Chest(rows) => {
                let rows = usize::from(rows);
                out.height = 114. + rows as f32 * 18.;
                out.inventory_label = [8., out.height - 94.];
                out.grid(0, 8., 18., 9, rows);
                player = (8., 31. + rows as f32 * 18.);
                player_start = rows * 9;
            }
            MenuKind::Dispenser => {
                out.grid(0, 62., 17., 3, 3);
                player_start = 9;
            }
            MenuKind::Crafter => {
                out.grid(0, 26., 17., 3, 3);
                out.slot(45, 134., 35.);
                player_start = 9;
            }
            MenuKind::Crafting => {
                out.slot(0, 124., 35.);
                out.grid(1, 30., 17., 3, 3);
                player_start = 10;
            }
            MenuKind::Anvil => {
                out.points(&[(27., 47.), (76., 47.), (134., 47.)]);
                player_start = 3;
            }
            MenuKind::Beacon => {
                out.width = 230.;
                out.height = 219.;
                out.inventory_label = [36., 125.];
                out.slot(0, 136., 110.);
                player = (36., 137.);
                player_start = 1;
            }
            MenuKind::Furnace | MenuKind::BlastFurnace | MenuKind::Smoker => {
                out.points(&[(56., 17.), (56., 53.), (116., 35.)]);
                player_start = 3;
            }
            MenuKind::BrewingStand => {
                out.points(&[(56., 51.), (79., 58.), (102., 51.), (79., 17.), (17., 17.)]);
                player_start = 5;
            }
            MenuKind::Enchantment => {
                out.points(&[(15., 47.), (35., 47.)]);
                player_start = 2;
            }
            MenuKind::Grindstone => {
                out.points(&[(49., 19.), (49., 40.), (129., 34.)]);
                player_start = 3;
            }
            MenuKind::Hopper => {
                out.height = 133.;
                out.inventory_label = [8., 39.];
                out.grid(0, 44., 20., 5, 1);
                player = (8., 51.);
                player_start = 5;
            }
            MenuKind::Loom => {
                out.points(&[(13., 26.), (33., 26.), (23., 45.), (143., 57.)]);
                player_start = 4;
            }
            MenuKind::Merchant => {
                out.width = 276.;
                out.inventory_label = [108., 72.];
                out.points(&[(136., 37.), (162., 37.), (220., 37.)]);
                player = (108., 84.);
                player_start = 3;
            }
            MenuKind::ShulkerBox => {
                out.grid(0, 8., 18., 9, 3);
                player_start = 27;
            }
            MenuKind::Smithing => {
                out.points(&[(8., 48.), (26., 48.), (44., 48.), (98., 48.)]);
                player_start = 4;
            }
            MenuKind::Cartography => {
                out.points(&[(15., 15.), (15., 52.), (145., 39.)]);
                player_start = 3;
            }
            MenuKind::Stonecutter => {
                out.points(&[(20., 33.), (143., 33.)]);
                player_start = 2;
            }
            MenuKind::Lectern => {
                out.width = 192.;
                out.height = 192.;
                return Ok(out);
            }
            MenuKind::Horse => {
                let columns = usize::from(m.horse_columns.ok_or("mount_columns_not_observed")?);
                if columns > 5 || m.slots.len() != 38 + columns * 3 {
                    return Err("mount_slot_layout_mismatch".into());
                }
                out.slot(0, 8., 18.);
                out.slot(1, 8., 36.);
                out.grid(2, 80., 18., columns, 3);
                player_start = 2 + columns * 3;
            }
        }
        out.standard(player_start, player.0, player.1);
        if out.slots.iter().any(|s| s.index >= m.slots.len()) {
            return Err("source menu layout exceeds observed slot count".into());
        }
        Ok(out)
    }
    fn slot(&mut self, index: usize, x: f32, y: f32) {
        self.slots.push(SlotRect { index, x, y });
    }
    fn points(&mut self, points: &[(f32, f32)]) {
        for (i, &(x, y)) in points.iter().enumerate() {
            self.slot(i, x, y);
        }
    }
    fn grid(&mut self, start: usize, x: f32, y: f32, columns: usize, rows: usize) {
        for row in 0..rows {
            for col in 0..columns {
                self.slot(
                    start + row * columns + col,
                    x + 18. * col as f32,
                    y + 18. * row as f32,
                );
            }
        }
    }
    fn standard(&mut self, start: usize, x: f32, y: f32) {
        self.grid(start, x, y, 9, 3);
        self.grid(start + 27, x, y + 58., 9, 1);
    }
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        self.slots
            .iter()
            .find(|s| x >= s.x - 1. && x < s.x + 17. && y >= s.y - 1. && y < s.y + 17.)
            .map(|s| s.index)
    }
}
