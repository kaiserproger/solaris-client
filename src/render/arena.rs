use std::ops::Range;

use super::packed::TerrainVertex;

/// Stride for vertex-arena alignment: MDI `base_vertex` divides the global
/// byte offset by this, so every vertex allocation offset must be a multiple.
const VERTEX_STRIDE: u64 = std::mem::size_of::<TerrainVertex>() as u64;
/// Index arenas only need 4-byte element alignment.
const INDEX_ALIGN: u64 = 4;

fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align > 0);
    value.div_ceil(align) * align
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Allocation {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug)]
pub(super) struct FreeList {
    pub capacity: u64,
    pub used: u64,
    free: Vec<Range<u64>>,
    retired: Vec<(u64, Range<u64>)>,
}
impl FreeList {
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity,
            used: 0,
            free: vec![0..capacity],
            retired: Vec::new(),
        }
    }
    pub fn allocate(&mut self, size: u64) -> Option<u64> {
        if size == 0 {
            return None;
        }
        let slot = self.free.iter().position(|r| r.end - r.start >= size)?;
        let offset = self.free[slot].start;
        self.free[slot].start += size;
        if self.free[slot].is_empty() {
            self.free.remove(slot);
        }
        self.used += size;
        Some(offset)
    }
    pub fn release(&mut self, offset: u64, size: u64) {
        assert!(size > 0 && offset <= self.capacity && size <= self.capacity - offset);
        let end = offset + size;
        let slot = self.free.partition_point(|r| r.start < offset);
        assert!(
            slot == 0 || self.free[slot - 1].end <= offset,
            "double/overlapping arena free"
        );
        assert!(
            slot == self.free.len() || end <= self.free[slot].start,
            "double/overlapping arena free"
        );
        self.free.insert(slot, offset..end);
        if slot + 1 < self.free.len() && self.free[slot].end == self.free[slot + 1].start {
            self.free[slot].end = self.free.remove(slot + 1).end;
        }
        if slot > 0 && self.free[slot - 1].end == offset {
            self.free[slot - 1].end = self.free.remove(slot).end;
        }
        self.used -= size;
    }
    /// Extend capacity without moving live offsets. Coalesces with a trailing
    /// free range so growth never fragments the free list.
    pub fn grow(&mut self, new_capacity: u64) {
        assert!(new_capacity >= self.capacity);
        if new_capacity == self.capacity {
            return;
        }
        let old = self.capacity;
        self.capacity = new_capacity;
        if let Some(last) = self.free.last_mut() {
            if last.end == old {
                last.end = new_capacity;
                return;
            }
        }
        self.free.push(old..new_capacity);
    }
    pub fn retire(&mut self, offset: u64, size: u64, submitted_frame: u64) {
        assert!(size > 0 && offset <= self.capacity && size <= self.capacity - offset);
        self.retired.push((submitted_frame, offset..offset + size));
    }
    pub fn reclaim(&mut self, completed_frame: u64) {
        let mut i = 0;
        while i < self.retired.len() {
            if self.retired[i].0 <= completed_frame {
                let (_, range) = self.retired.swap_remove(i);
                self.release(range.start, range.end - range.start);
            } else {
                i += 1;
            }
        }
    }
    pub fn retired_bytes(&self) -> u64 {
        self.retired
            .iter()
            .map(|(_, range)| range.end - range.start)
            .sum()
    }
    pub fn largest_free(&self) -> u64 {
        self.free.iter().map(|r| r.end - r.start).max().unwrap_or(0)
    }
}

/// One growable GPU buffer shared by every allocation. A single buffer lets
/// the terrain pass bind once per material and issue ONE multi-draw-indirect
/// call instead of one draw per page.
pub(super) struct Arena {
    buffer: Option<wgpu::Buffer>,
    free: FreeList,
    budget: u64,
    page_size: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}
impl Arena {
    pub fn new(
        budget: u64,
        page_size: u64,
        usage: wgpu::BufferUsages,
        label: &'static str,
    ) -> Self {
        Self {
            buffer: None,
            free: FreeList::new(0),
            budget,
            page_size,
            usage,
            label,
        }
    }
    fn stride(&self) -> u64 {
        if self.usage.contains(wgpu::BufferUsages::VERTEX) {
            VERTEX_STRIDE
        } else {
            INDEX_ALIGN
        }
    }
    /// Pure growth math (no GPU): exponential within `min(budget,
    /// max_buffer)`, always stride-aligned so vertex offsets stay divisible
    /// by the MDI `base_vertex` stride. Errors explicitly instead of
    /// silently narrowing the configured budget.
    fn next_capacity(&self, size: u64, max_buffer: u64) -> Result<u64, String> {
        let align = self.stride();
        let old = self.free.capacity;
        let tail = self
            .free
            .free
            .last()
            .filter(|range| range.end == old)
            .map_or(0, |range| range.end - range.start);
        // Interior holes cannot satisfy a contiguous new allocation. Grow the
        // trailing range, not merely the sum of live allocation byte counts.
        let need = old
            .checked_add(size.saturating_sub(tail))
            .ok_or("mesh_allocation_overflow")?;
        if need > self.budget {
            return Err(format!(
                "{}_budget_exceeded requested={size} reserve={old} budget={}",
                self.label, self.budget
            ));
        }
        if need > max_buffer {
            return Err(format!(
                "{}_device_buffer_limit_exceeded requested={size} reserve={old} limit={max_buffer}",
                self.label
            ));
        }
        let size_pow2 = size
            .checked_next_power_of_two()
            .ok_or("mesh_allocation_overflow")?;
        let mut grown = old.max(self.page_size).max(size_pow2);
        if grown <= old {
            // Fragmented but fully reserved: must actually add space.
            grown = old.checked_mul(2).ok_or("mesh_allocation_overflow")?;
        }
        while grown < need {
            grown = grown.checked_mul(2).ok_or("mesh_allocation_overflow")?;
        }
        grown = align_up(grown, align);
        let limit = self.budget.min(max_buffer);
        if grown > limit {
            // Use the final allowed capacity once, not tiny per-mesh growth
            // copies when the exponential size first crosses the budget.
            grown = limit / align * align;
            if grown < need {
                return Err(format!("{}_aligned_buffer_budget_exceeded", self.label));
            }
        }
        Ok(grown)
    }
    /// Allocate `size` bytes. Grows the single buffer lazily; old contents
    /// are preserved by a queue-ordered GPU copy submitted before any later
    /// write that uses the new buffer.
    pub fn allocate(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: u64,
    ) -> Result<Option<Allocation>, String> {
        if size == 0 {
            return Ok(None);
        }
        let stride = self.stride();
        if !size.is_multiple_of(stride) {
            return Err("unaligned_mesh_allocation".into());
        }
        if let Some(offset) = self.free.allocate(size) {
            debug_assert_eq!(offset % stride, 0, "misaligned arena offset");
            return Ok(Some(Allocation { offset, size }));
        }
        let max_buffer = device.limits().max_buffer_size;
        let grown = self.next_capacity(size, max_buffer)?;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: grown,
            usage: self.usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if let Some(old) = self.buffer.take() {
            let old_capacity = self.free.capacity;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("arena grow copy"),
            });
            encoder.copy_buffer_to_buffer(&old, 0, &buffer, 0, old_capacity);
            let _ = queue.submit([encoder.finish()]);
        }
        self.buffer = Some(buffer);
        self.free.grow(grown);
        let offset = self
            .free
            .allocate(size)
            .expect("arena growth must satisfy allocation");
        debug_assert_eq!(offset % stride, 0, "misaligned arena offset");
        Ok(Some(Allocation { offset, size }))
    }
    pub fn release(&mut self, allocation: Allocation) {
        self.free.release(allocation.offset, allocation.size);
    }
    pub fn retire(&mut self, allocation: Allocation, submitted_frame: u64) {
        self.free
            .retire(allocation.offset, allocation.size, submitted_frame);
    }
    pub fn reclaim(&mut self, completed_frame: u64) {
        self.free.reclaim(completed_frame);
    }
    pub fn retired_bytes(&self) -> u64 {
        self.free.retired_bytes()
    }
    pub fn write(&self, queue: &wgpu::Queue, allocation: Allocation, data: &[u8]) {
        assert_eq!(allocation.size, data.len() as u64);
        queue.write_buffer(
            self.buffer
                .as_ref()
                .expect("arena buffer used before any allocation"),
            allocation.offset,
            data,
        );
    }
    pub fn slice(&self, a: Allocation) -> wgpu::BufferSlice<'_> {
        self.buffer
            .as_ref()
            .expect("arena buffer used before any allocation")
            .slice(a.offset..a.offset + a.size)
    }
    /// Entire shared buffer for the ONE multi-draw-indirect call per terrain
    /// material. Only valid after at least one allocation has created it.
    pub fn whole(&self) -> wgpu::BufferSlice<'_> {
        self.buffer
            .as_ref()
            .expect("arena buffer used before any allocation")
            .slice(..)
    }
    pub fn reserved(&self) -> u64 {
        self.free.capacity
    }
    pub fn used(&self) -> u64 {
        self.free.used
    }
    pub fn largest_free(&self) -> u64 {
        self.free.largest_free()
    }
}

#[cfg(test)]
mod tests {
    use super::Arena;
    use super::FreeList;
    #[test]
    fn freed_neighbors_coalesce_and_reuse_without_aliasing_live_allocations() {
        let mut arena = FreeList::new(128);
        let a = arena.allocate(32).unwrap();
        let b = arena.allocate(48).unwrap();
        let c = arena.allocate(48).unwrap();
        assert!(arena.allocate(4).is_none());
        arena.release(b, 48);
        arena.release(a, 32);
        assert_eq!(arena.allocate(80), Some(0));
        assert_eq!(c, 80);
        assert_eq!(arena.used, 128);
        arena.release(c, 48);
        arena.release(0, 80);
        assert_eq!(arena.largest_free(), 128);
    }
    #[test]
    #[should_panic(expected = "double/overlapping")]
    fn overlap_is_rejected_before_reusing_live_bytes() {
        let mut arena = FreeList::new(64);
        arena.allocate(32).unwrap();
        arena.release(0, 32);
        arena.release(0, 32);
    }
    #[test]
    fn submitted_allocations_are_not_reused_until_their_completion_fence() {
        let mut arena = FreeList::new(128);
        let a = arena.allocate(64).unwrap();
        let b = arena.allocate(64).unwrap();
        arena.retire(a, 64, 5);
        arena.retire(b, 64, 7);
        arena.reclaim(4);
        assert_eq!(arena.allocate(4), None);
        assert_eq!(arena.retired_bytes(), 128);
        arena.reclaim(5);
        assert_eq!(arena.allocate(64), Some(a));
        assert_eq!(arena.allocate(4), None);
        assert_eq!(arena.retired_bytes(), 64);
        arena.reclaim(7);
        assert_eq!(arena.allocate(64), Some(b));
        assert_eq!(arena.retired_bytes(), 0);
    }
    #[test]
    fn growth_preserves_live_offsets_and_coalesces_trailing_free() {
        let mut arena = Arena::new(1024, 128, wgpu::BufferUsages::INDEX, "test_index");
        let grown = arena.next_capacity(64, u64::MAX).unwrap();
        assert!(grown >= 128);
        arena.free.grow(grown);
        let a = arena.free.allocate(32).unwrap();
        let live = arena.free.allocate(32).unwrap();
        assert_eq!((a, live), (0, 32));
        arena.free.release(a, 32);
        let regrown = arena.next_capacity(32, u64::MAX).unwrap();
        assert!(regrown > grown);
        arena.free.grow(regrown);
        // Live allocation offset is untouched by the move.
        assert_eq!(live, 32);
        // Trailing free range absorbed the new space contiguously.
        assert!(arena.free.largest_free() >= regrown - 64);
        arena.free.release(live, 32);
        assert_eq!(arena.free.largest_free(), regrown);
    }
    #[test]
    fn vertex_offsets_stay_stride_aligned_across_growth() {
        let stride = std::mem::size_of::<crate::render::packed::TerrainVertex>() as u64;
        assert!(stride.is_multiple_of(4));
        let mut arena = Arena::new(
            1024 * 1024 * 1024,
            4 * 1024 * 1024,
            wgpu::BufferUsages::VERTEX,
            "test_vertex",
        );
        let grown = arena.next_capacity(stride * 3, u64::MAX).unwrap();
        assert_eq!(grown % stride, 0, "growth must preserve stride alignment");
        arena.free.grow(grown);
        let head = arena.free.allocate(stride * 3).unwrap();
        let live = arena.free.allocate(stride).unwrap();
        assert_eq!(head % stride, 0);
        assert_eq!(live % stride, 0);
        arena.free.release(head, stride * 3);
        let regrown = arena.next_capacity(grown, u64::MAX).unwrap();
        assert!(regrown > grown);
        assert_eq!(regrown % stride, 0);
        arena.free.grow(regrown);
        assert_eq!(live, stride * 3, "growth must not move live offsets");
        assert_eq!(live % stride, 0);
    }
    #[test]
    fn growth_respects_budget_and_device_limits() {
        let arena = Arena::new(1024, 512, wgpu::BufferUsages::INDEX, "test_budget");
        assert!(arena.next_capacity(512, u64::MAX).is_ok());
        let over_budget = arena.next_capacity(2048, u64::MAX).unwrap_err();
        assert!(over_budget.contains("budget_exceeded"), "{over_budget}");
        let over_device = arena.next_capacity(512, 256).unwrap_err();
        assert!(
            over_device.contains("device_buffer_limit_exceeded"),
            "{over_device}"
        );
    }

    #[test]
    fn fragmented_growth_reserves_contiguous_space_not_just_used_bytes() {
        let mut arena = Arena::new(152, 100, wgpu::BufferUsages::INDEX, "fragmented");
        arena.free.grow(100);
        arena.free.allocate(40).unwrap();
        arena.free.allocate(60).unwrap();
        arena.free.release(0, 40);
        assert!(arena.free.allocate(52).is_none());
        let grown = arena.next_capacity(52, u64::MAX).unwrap();
        arena.free.grow(grown);
        assert_eq!(arena.free.allocate(52), Some(100));
    }

    #[test]
    fn final_capped_growth_uses_all_available_space_once() {
        let mut arena = Arena::new(200, 128, wgpu::BufferUsages::INDEX, "capped");
        arena.free.grow(128);
        arena.free.allocate(128).unwrap();
        assert_eq!(arena.next_capacity(4, u64::MAX).unwrap(), 200);
    }
}
