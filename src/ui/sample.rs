use crate::protocol::Result;
/// Source SlotDisplay.generate.limit(256).filter(nonempty).limit(16), with identity-seeded thread-local legacy RNG.
pub(super) fn demo<T>(
    seed: i32,
    base: usize,
    material: usize,
    mut transform: impl FnMut(usize, usize) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    if base == 0 || material == 0 {
        return Ok(Vec::new());
    }
    if base > i32::MAX as usize || material > i32::MAX as usize {
        return Err("recipe_sample_bound".into());
    }
    let mut random = LegacyRandom::new(i64::from(seed));
    let mut values = Vec::with_capacity(16);
    for _ in 0..256 {
        let i = random.index(base);
        let j = random.index(material);
        if let Some(value) = transform(i, j)? {
            values.push(value);
        }
        if values.len() == 16 {
            break;
        }
    }
    Ok(values)
}
struct LegacyRandom(u64);
impl LegacyRandom {
    fn new(seed: i64) -> Self {
        Self((seed as u64 ^ 0x5deece66d) & ((1 << 48) - 1))
    }
    fn bits(&mut self, bits: u32) -> u32 {
        self.0 = (self.0.wrapping_mul(0x5deece66d).wrapping_add(11)) & ((1 << 48) - 1);
        (self.0 >> (48 - bits)) as u32
    }
    fn index(&mut self, bound: usize) -> usize {
        if bound.is_power_of_two() {
            return ((bound as u64 * u64::from(self.bits(31))) >> 31) as usize;
        }
        loop {
            let bits = self.bits(31);
            let value = bits % bound as u32;
            if (bits.wrapping_sub(value).wrapping_add(bound as u32 - 1) as i32) >= 0 {
                return value as usize;
            }
        }
    }
}
