use super::{Section, block_index};
use crate::protocol::Result;
use std::sync::Arc;

#[derive(Clone)]
pub struct SurfaceHeightmap {
    words: Vec<u64>,
    bits: u8,
    per_word: usize,
    mask: u64,
}
impl SurfaceHeightmap {
    pub fn new(height: i32) -> Result<Self> {
        let (bits, per_word, mask) = layout(height)?;
        Ok(Self {
            words: vec![0; 256usize.div_ceil(per_word)],
            bits,
            per_word,
            mask,
        })
    }
    pub(super) fn received(
        height: i32,
        words: Option<Vec<u64>>,
        sections: &[Arc<Section>],
        mut is_air: impl FnMut(u32) -> Result<bool>,
    ) -> Result<Self> {
        if let Some(words) = words {
            let (bits, per_word, mask) = layout(height)?;
            let expected = 256usize.div_ceil(per_word);
            if words.len() == expected {
                return Ok(Self {
                    words,
                    bits,
                    per_word,
                    mask,
                });
            }
            eprintln!(
                "Ignoring WORLD_SURFACE heightmap data: expected {} longs, got {}; priming from received blocks",
                expected,
                words.len()
            );
            let mut out = Self::new(height)?;
            let top = sections
                .iter()
                .rposition(|section| section.non_empty_block_count != 0)
                .map_or(16, |section| (section + 1) * 16);
            for x in 0..16 {
                for z in 0..16 {
                    for y in (0..top).rev() {
                        let state = sections[y / 16].blocks.get(block_index([x, y as i32, z]));
                        if !is_air(state)? {
                            out.set_offset([x, z], y as u32 + 1);
                            break;
                        }
                    }
                }
            }
            return Ok(out);
        }
        let out = Self::new(height)?;
        Ok(out)
    }
    pub fn first_available(&self, position: [i32; 2], min_y: i32) -> i32 {
        min_y + self.offset(position) as i32
    }
    pub(super) fn next_height(
        &self,
        position: [i32; 3],
        min_y: i32,
        air: bool,
        mut air_below: impl FnMut(i32) -> Result<bool>,
    ) -> Result<Option<u32>> {
        let column = [position[0], position[2]];
        let first = self.first_available(column, min_y);
        if position[1] <= first - 2 {
            return Ok(None);
        }
        let next = if !air {
            if position[1] < first {
                return Ok(None);
            }
            position[1] + 1
        } else if first - 1 == position[1] {
            let mut y = position[1] - 1;
            while y >= min_y && air_below(y)? {
                y -= 1;
            }
            y + 1
        } else {
            return Ok(None);
        };
        Ok(Some((next - min_y) as u32))
    }
    fn offset(&self, position: [i32; 2]) -> u32 {
        let index = ((position[0] & 15) + (position[1] & 15) * 16) as usize;
        ((self.words[index / self.per_word] >> ((index % self.per_word) * usize::from(self.bits)))
            & self.mask) as u32
    }
    pub(super) fn set_offset(&mut self, position: [i32; 2], value: u32) {
        let index = ((position[0] & 15) + (position[1] & 15) * 16) as usize;
        let shift = (index % self.per_word) * usize::from(self.bits);
        let word = &mut self.words[index / self.per_word];
        *word = (*word & !(self.mask << shift)) | (u64::from(value) << shift);
    }
    pub(super) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.words.capacity() * 8
    }
}
fn layout(height: i32) -> Result<(u8, usize, u64)> {
    if height <= 0 {
        return Err("heightmap dimension height must be positive".into());
    }
    let bits = (32 - (height as u32).leading_zeros()) as u8;
    Ok((bits, 64 / usize::from(bits), (1u64 << bits) - 1))
}
