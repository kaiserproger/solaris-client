use serde_json::{json, Value};
use std::{cell::Cell, sync::mpsc};

pub(super) const PASSES: [&str; 21] = [
    "sky",
    "opaque",
    "cutout",
    "translucent",
    "outline_mask",
    "outline_silhouette",
    "outline_horizontal_blur",
    "outline_vertical_blur",
    "outline_identity_blit",
    "outline_rgb_composite",
    "ui_overlay",
    "present_blit",
    "directional_shadow",
    "bloom_horizontal",
    "bloom_vertical",
    "aces_tonemap",
    "terrain_frustum_cull",
    "hiz_pyramid_build",
    "terrain_occlusion_cull",
    "terrain_occlusion_draw",
    "dynamic_geometry",
];
const QUERIES: u32 = PASSES.len() as u32 * 2;
const RESOLVE_STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
struct Slot {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    pending: Option<(u64, u32, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>)>,
}
pub(super) struct Timing {
    slots: Vec<Slot>,
    active: Option<usize>,
    used: Cell<u32>,
    period: f64,
    latest_frame: u64,
    pub latest: Value,
    pub missed_samples: u64,
}
impl Timing {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, supported: bool) -> Self {
        let slots = if supported {
            (0..3)
                .map(|_| Slot {
                    queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("frame pass timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: QUERIES,
                    }),
                    resolve: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("timestamp resolve"),
                        size: PASSES.len() as u64 * RESOLVE_STRIDE,
                        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    }),
                    readback: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("async timestamp readback"),
                        size: PASSES.len() as u64 * RESOLVE_STRIDE,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    pending: None,
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            slots,
            active: None,
            used: Cell::new(0),
            period: f64::from(queue.get_timestamp_period()),
            missed_samples: 0,
            latest_frame: 0,
            latest: json!({"status":if supported {"awaiting_completed_sample"} else {"unsupported_adapter_feature"},"frame_id":null,"total_ms":null,"passes_ms":null}),
        }
    }
    pub fn begin(&mut self) {
        self.active = self.slots.iter().position(|s| s.pending.is_none());
        self.used.set(0);
        if self.active.is_none() && !self.slots.is_empty() {
            self.missed_samples += 1;
        }
    }
    pub fn pass_mask(&self) -> u32 {
        self.used.get()
    }
    pub fn writes(&self, pass: usize) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.used.set(self.used.get() | (1 << pass));
        self.active.map(|slot| wgpu::RenderPassTimestampWrites {
            query_set: &self.slots[slot].queries,
            beginning_of_pass_write_index: Some(pass as u32 * 2),
            end_of_pass_write_index: Some(pass as u32 * 2 + 1),
        })
    }
    pub fn compute_writes(&self, pass: usize) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        self.used.set(self.used.get() | (1 << pass));
        self.active.map(|slot| wgpu::ComputePassTimestampWrites {
            query_set: &self.slots[slot].queries,
            beginning_of_pass_write_index: Some(pass as u32 * 2),
            end_of_pass_write_index: Some(pass as u32 * 2 + 1),
        })
    }
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(i) = self.active {
            let slot = &self.slots[i];
            // Resolve only written pairs. Unwritten queries must not wait for a
            // skipped outline stage, or report stale timestamps from this slot.
            for pass in 0..PASSES.len() {
                if self.used.get() & (1 << pass) != 0 {
                    encoder.resolve_query_set(
                        &slot.queries,
                        pass as u32 * 2..pass as u32 * 2 + 2,
                        &slot.resolve,
                        pass as u64 * RESOLVE_STRIDE,
                    );
                }
            }
            encoder.copy_buffer_to_buffer(
                &slot.resolve,
                0,
                &slot.readback,
                0,
                PASSES.len() as u64 * RESOLVE_STRIDE,
            );
        }
    }
    pub fn submitted(&mut self, frame: u64) {
        if let Some(i) = self.active.take() {
            let slot = &mut self.slots[i];
            let (tx, rx) = mpsc::sync_channel(1);
            slot.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let _ = tx.send(r);
                });
            slot.pending = Some((frame, self.used.get(), rx));
        }
    }
    pub fn collect(&mut self) {
        for slot in &mut self.slots {
            let Some((frame, used, rx)) = &slot.pending else {
                continue;
            };
            let Ok(result) = rx.try_recv() else { continue };
            let frame = *frame;
            let used = *used;
            if frame < self.latest_frame {
                if result.is_ok() {
                    slot.readback.unmap();
                }
                slot.pending = None;
                continue;
            }
            self.latest_frame = frame;
            if result.is_ok() {
                let bytes = slot.readback.slice(..).get_mapped_range();
                let mut values = [0u64; QUERIES as usize];
                for (i, pair) in bytes.chunks_exact(RESOLVE_STRIDE as usize).enumerate() {
                    values[i * 2] = u64::from_le_bytes(pair[..8].try_into().unwrap());
                    values[i * 2 + 1] = u64::from_le_bytes(pair[8..16].try_into().unwrap());
                }
                let mut passes = serde_json::Map::new();
                let mut total = 0.;
                let mut first_timestamp = u64::MAX;
                let mut last_timestamp = 0;
                for (i, pass) in PASSES.iter().enumerate() {
                    if used & (1 << i) == 0 {
                        passes.insert((*pass).into(), Value::Null);
                        continue;
                    }
                    first_timestamp = first_timestamp.min(values[i * 2]);
                    last_timestamp = last_timestamp.max(values[i * 2 + 1]);
                    let duration = values[i * 2 + 1].saturating_sub(values[i * 2]) as f64
                        * self.period
                        / 1_000_000.;
                    total += duration;
                    passes.insert((*pass).into(), json!(duration));
                }
                self.latest = json!({"status":"measured_completed","frame_id":frame.to_string(),"total_ms":total,"passes_ms":passes,
                    "frame_span_ms":last_timestamp.saturating_sub(first_timestamp) as f64 * self.period / 1_000_000.,
                    "frame_span_measurement":"first_to_last_render_or_compute_pass_includes_interpass_depth_copy_excludes_queue_wait_and_capture_copy",
                    "timestamp_period_ns":self.period,"measurement":"sum_of_render_and_compute_pass_device_timestamps_excludes_queue_wait_and_capture_copy"});
                drop(bytes);
                slot.readback.unmap();
            } else {
                self.latest = json!({"status":"readback_failed","frame_id":frame.to_string(),"total_ms":null,"passes_ms":null});
            }
            slot.pending = None;
        }
    }
}

/// Thread CPU time distinguishes an X11/driver wait in present() from actual
/// work. No allocation or procfs walk on the render thread. Other platforms
/// keep the existing wall metric and report CPU time as unavailable.
#[cfg(target_os = "linux")]
pub(super) fn thread_cpu_seconds() -> Option<f64> {
    #[repr(C)]
    #[derive(Default)]
    struct Timespec {
        seconds: std::os::raw::c_long,
        nanoseconds: std::os::raw::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: std::os::raw::c_int, time: *mut Timespec) -> std::os::raw::c_int;
    }
    let mut time = Timespec::default();
    // SAFETY: Linux CLOCK_THREAD_CPUTIME_ID=3; Timespec has the libc ABI,
    // and clock_gettime writes into this live, aligned, exclusive stack value.
    (unsafe { clock_gettime(3, &mut time) } == 0)
        .then_some(time.seconds as f64 + time.nanoseconds as f64 * 1e-9)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn thread_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn thread_cpu_clock_is_available_and_monotonic_without_wall_waits() {
    let start = thread_cpu_seconds().unwrap();
    for value in 0..10000 {
        std::hint::black_box(value);
    }
    let end = thread_cpu_seconds().unwrap();
    assert!(end.is_finite() && end >= start);
}
