use super::arena::{Allocation, Arena};
use crate::api::Vertex;
use std::ops::Range;

pub(super) const VERTEX_BUDGET: u64 = 1024 * 1024 * 1024;
pub(super) const INDEX_BUDGET: u64 = 512 * 1024 * 1024;
pub(super) const MAX_CHUNKS: usize = 32768;
pub(super) const MAX_TRANSPARENT_PRIMITIVES: usize = 1_048_576;

#[derive(Debug)]
pub(super) struct ResidentMesh {
    pub vertices: Option<Allocation>,
    pub indices: Option<Allocation>,
    pub ranges: [Range<u32>; 3],
    pub transparent: Translucent,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Primitive {
    pub center: [f32; 3],
    pub first: u32,
    pub count: u32,
    distance: f32,
}

/// CPU-owned quad topology and reusable index output for one terrain section.
/// Dynamic entity primitives retain their independent per-material sort path.
#[derive(Debug, Default)]
pub(super) struct Translucent {
    primitives: Vec<Primitive>,
    source_indices: Vec<u32>,
    sorted_indices: Vec<u32>,
    last_camera: Option<[f64; 3]>,
}

impl Translucent {
    fn new(vertices: &[Vertex], indices: &[u32]) -> Result<Self, String> {
        Ok(Self {
            primitives: primitives(vertices, indices, 0)?,
            source_indices: indices.to_vec(),
            sorted_indices: Vec::with_capacity(indices.len()),
            last_camera: None,
        })
    }

    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    fn sort(&mut self, camera: [f64; 3]) -> bool {
        if self.primitives.is_empty() {
            return false;
        }
        if let Some(last) = self.last_camera {
            let distance = (0..3)
                .map(|axis| (camera[axis] - last[axis]).powi(2))
                .sum::<f64>();
            let crossed_section =
                (0..3).any(|axis| (camera[axis] / 16.).floor() != (last[axis] / 16.).floor());
            if distance <= 1. && !crossed_section {
                return false;
            }
        }
        for primitive in &mut self.primitives {
            primitive.distance = (0..3)
                .map(|axis| (f64::from(primitive.center[axis]) - camera[axis]).powi(2))
                .sum::<f64>() as f32;
        }
        self.primitives.sort_unstable_by(|a, b| {
            b.distance
                .total_cmp(&a.distance)
                .then_with(|| a.first.cmp(&b.first))
        });
        self.sorted_indices.clear();
        for primitive in &self.primitives {
            let first = primitive.first as usize;
            self.sorted_indices
                .extend_from_slice(&self.source_indices[first..first + primitive.count as usize]);
        }
        self.last_camera = Some(camera);
        true
    }
}

pub(super) fn validate(vertices: &[Vertex], indices: &[&[u32]], layers: u32) -> Result<(), String> {
    if vertices.len() > i32::MAX as usize {
        return Err("mesh_vertex_count_limit".into());
    }
    if let Some((index, vertex)) = vertices.iter().enumerate().find(|(_, v)| {
        v.texture_layer >= layers
            || !v.position.iter().all(|x| x.is_finite())
            || !v.normal.iter().all(|x| x.is_finite())
            || !v.alpha.is_finite()
            || v.alpha < 0.
            || v.alpha > 1.
            || (v.light != [-1.; 2]
                && !v
                    .light
                    .iter()
                    .all(|x| x.is_finite() && *x >= 0. && *x <= 15.))
    }) {
        return Err(format!(
            "invalid_mesh_vertex_or_texture_layer: index={index} layer={} layers={layers} position={:?} normal={:?} alpha={} light={:?}",
            vertex.texture_layer, vertex.position, vertex.normal, vertex.alpha, vertex.light
        ));
    }
    if indices.iter().map(|i| i.len() as u64).sum::<u64>() > u64::from(u32::MAX) {
        return Err("mesh_total_index_count_limit".into());
    }
    for layer in indices {
        if layer.len() > u32::MAX as usize
            || layer.len() % 3 != 0
            || layer.iter().any(|&i| i as usize >= vertices.len())
        {
            return Err("invalid_mesh_indices".into());
        }
    }
    Ok(())
}

pub(super) fn primitives(
    vertices: &[Vertex],
    indices: &[u32],
    first: u32,
) -> Result<Vec<Primitive>, String> {
    let mut result = Vec::with_capacity(indices.len() / 6);
    primitives_into(vertices, indices, first, &mut result)?;
    Ok(result)
}

fn primitives_into(
    vertices: &[Vertex],
    indices: &[u32],
    first: u32,
    result: &mut Vec<Primitive>,
) -> Result<(), String> {
    result.clear();
    let mut i = 0;
    while i < indices.len() {
        // Preserve each baker quad as the sorting primitive, not its two halves.
        let quad = i + 6 <= indices.len()
            && indices[i] == indices[i + 3]
            && indices[i + 2] == indices[i + 4];
        let count = if quad { 6 } else { 3 };
        let mut center = [0.; 3];
        if quad {
            // MeshData.unpackQuadCentroids uses opposite vertices 0 and 2.
            // Averaging all four differs for deformed/non-planar Source quads.
            let a = vertices[indices[i] as usize].position;
            let b = vertices[indices[i + 2] as usize].position;
            for axis in 0..3 {
                center[axis] = (a[axis] + b[axis]) / 2.;
            }
        } else {
            for &index in &indices[i..i + 3] {
                for axis in 0..3 {
                    center[axis] += vertices[index as usize].position[axis] / 3.;
                }
            }
        }
        result.push(Primitive {
            center,
            first: first + i as u32,
            count,
            distance: 0.,
        });
        if result.len() > MAX_TRANSPARENT_PRIMITIVES {
            return Err("transparent_primitive_budget_exceeded".into());
        }
        i += count as usize;
    }
    Ok(())
}

impl ResidentMesh {
    pub fn reserve(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        va: &mut Arena,
        ia: &mut Arena,
        vertices: &[Vertex],
        indices: [&[u32]; 3],
    ) -> Result<Self, String> {
        let mut count = 0u32;
        let ranges = std::array::from_fn(|layer| {
            let first = count;
            count += indices[layer].len() as u32;
            first..count
        });
        let transparent = Translucent::new(vertices, indices[2])?;
        let vertices = va.allocate(device, queue, std::mem::size_of_val(vertices) as u64)?;
        let index_allocation = match ia.allocate(device, queue, u64::from(count) * 4) {
            Ok(a) => a,
            Err(e) => {
                if let Some(a) = vertices {
                    va.release(a);
                }
                return Err(e);
            }
        };
        // Writes are issued by upload_geometry after reservation; no partial
        // replacement can expose an allocation with absent indices.
        let mesh = Self {
            vertices,
            indices: index_allocation,
            ranges,
            transparent,
        };
        Ok(mesh)
    }
    pub fn sort_transparent(&mut self, queue: &wgpu::Queue, ia: &Arena, camera: [f64; 3]) -> u64 {
        if !self.transparent.sort(camera) {
            return 0;
        }
        if let Some(allocation) = self.indices {
            let bytes = bytemuck::cast_slice(&self.transparent.sorted_indices);
            ia.write(
                queue,
                Allocation {
                    offset: allocation.offset + u64::from(self.ranges[2].start) * 4,
                    size: bytes.len() as u64,
                },
                bytes,
            );
            return bytes.len() as u64;
        }
        0
    }
    pub fn upload_geometry(
        &self,
        queue: &wgpu::Queue,
        va: &Arena,
        ia: &Arena,
        vertices: &[Vertex],
        indices: [&[u32]; 3],
    ) {
        if let Some(a) = self.vertices {
            va.write(queue, a, bytemuck::cast_slice(vertices));
        }
        if let Some(a) = self.indices {
            // Queue writes can address disjoint material ranges without flattening.
            for layer in 0..3 {
                let bytes: &[u8] = bytemuck::cast_slice(indices[layer]);
                if !bytes.is_empty() {
                    ia.write(
                        queue,
                        Allocation {
                            offset: a.offset + u64::from(self.ranges[layer].start) * 4,
                            size: bytes.len() as u64,
                        },
                        bytes,
                    );
                }
            }
        }
    }
    pub fn bytes(&self) -> u64 {
        self.vertices.map_or(0, |a| a.size) + self.indices.map_or(0, |a| a.size)
    }
    pub fn release(self, va: &mut Arena, ia: &mut Arena) {
        if let Some(a) = self.vertices {
            va.release(a);
        }
        if let Some(a) = self.indices {
            ia.release(a);
        }
    }
    pub fn retire(self, va: &mut Arena, ia: &mut Arena, submitted_frame: u64) {
        if let Some(a) = self.vertices {
            va.retire(a, submitted_frame);
        }
        if let Some(a) = self.indices {
            ia.retire(a, submitted_frame);
        }
    }
}

pub(super) struct Stream<const N: usize = 3> {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
    pub ranges: [Range<u32>; N],
    pub transparent: [Vec<Primitive>; N],
    pub uploaded_bytes: u64,
}
impl<const N: usize> Stream<N> {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            vertices: buffer(device, 4, wgpu::BufferUsages::VERTEX),
            indices: buffer(device, 4, wgpu::BufferUsages::INDEX),
            vertex_capacity: 4,
            index_capacity: 4,
            ranges: std::array::from_fn(|_| 0..0),
            transparent: std::array::from_fn(|_| Vec::new()),
            uploaded_bytes: 0,
        }
    }
    pub fn validate_size(
        vertices: &[Vertex],
        indices: &[Vec<u32>; N],
        layers: u32,
    ) -> Result<(), String> {
        validate(vertices, &indices.each_ref().map(Vec::as_slice), layers)?;
        let vertex_bytes = std::mem::size_of_val(vertices) as u64;
        let index_bytes = indices.iter().map(|i| i.len() as u64 * 4).sum::<u64>();
        if vertex_bytes > 32 * 1024 * 1024
            || index_bytes > 16 * 1024 * 1024
            || indices.iter().map(Vec::len).sum::<usize>() > MAX_TRANSPARENT_PRIMITIVES * 3
        {
            return Err("dynamic_or_ui_stream_budget_exceeded".into());
        }
        Ok(())
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[Vertex],
        indices: &[Vec<u32>; N],
        sort: [bool; N],
    ) -> Result<(), String> {
        // sync_scene validates both streams before reserving terrain replacements.
        let vertex_bytes = std::mem::size_of_val(vertices) as u64;
        let index_bytes = indices.iter().map(|i| i.len() as u64 * 4).sum::<u64>();
        if vertex_bytes > self.vertex_capacity {
            self.vertex_capacity = vertex_bytes.next_power_of_two();
            self.vertices = buffer(device, self.vertex_capacity, wgpu::BufferUsages::VERTEX);
        }
        if index_bytes > self.index_capacity {
            self.index_capacity = index_bytes.next_power_of_two();
            self.indices = buffer(device, self.index_capacity, wgpu::BufferUsages::INDEX);
        }
        if vertex_bytes > 0 {
            queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
        }
        let mut first = 0;
        for layer in 0..N {
            let count = indices[layer].len() as u32;
            self.ranges[layer] = first..first + count;
            if count > 0 {
                queue.write_buffer(
                    &self.indices,
                    u64::from(first) * 4,
                    bytemuck::cast_slice(&indices[layer]),
                );
            }
            first += count;
            self.transparent[layer].clear();
            if sort[layer] {
                primitives_into(
                    vertices,
                    &indices[layer],
                    self.ranges[layer].start,
                    &mut self.transparent[layer],
                )?;
            }
        }
        self.uploaded_bytes = vertex_bytes + index_bytes;
        Ok(())
    }
    pub fn reserved(&self) -> u64 {
        self.vertex_capacity + self.index_capacity
    }
}
fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bounded presentation stream"),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn translucent_quads_are_not_split_into_independently_sorted_halves() {
        let mut vertices = vec![Vertex::zeroed(); 4];
        use bytemuck::Zeroable;
        vertices[0].position = [0., 0., 0.];
        vertices[1].position = [5., 0., 0.];
        vertices[2].position = [2., 2., 0.];
        vertices[3].position = [0., 5., 0.];
        let p = primitives(&vertices, &[0, 1, 2, 0, 2, 3], 12).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].center, [1., 1., 0.]);
        assert_eq!((p[0].first, p[0].count), (12, 6));
    }

    fn translucent_fixture() -> (Vec<Vertex>, Vec<u32>) {
        use bytemuck::Zeroable;
        let mut vertices = vec![Vertex::zeroed(); 11];
        for (index, vertex) in vertices.iter_mut().enumerate() {
            vertex.position = [
                if index < 4 {
                    2.
                } else if index < 8 {
                    12.
                } else {
                    7.
                },
                0.,
                0.,
            ];
        }
        (vertices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7, 8, 9, 10])
    }

    #[test]
    fn terrain_translucency_reorders_whole_quads_and_triangle_without_topology_loss() {
        let (vertices, indices) = translucent_fixture();
        let mut sorted = Translucent::new(&vertices, &indices).unwrap();
        assert!(sorted.sort([0.; 3]));
        assert_eq!(
            sorted.sorted_indices,
            [4, 5, 6, 4, 6, 7, 8, 9, 10, 0, 1, 2, 0, 2, 3]
        );
        let allocation = sorted.sorted_indices.as_ptr();
        assert!(sorted.sort([16., 0., 0.]));
        assert_eq!(
            sorted.sorted_indices,
            [0, 1, 2, 0, 2, 3, 8, 9, 10, 4, 5, 6, 4, 6, 7]
        );
        assert_eq!(sorted.source_indices, indices);
        assert_eq!(
            sorted.sorted_indices.as_ptr(),
            allocation,
            "sorting reuses the CPU index allocation"
        );
        assert!(sorted.sort([7., 0., 0.]));
        assert_eq!(
            sorted.sorted_indices, indices,
            "equal-distance ties retain source order"
        );
    }

    #[test]
    fn terrain_translucency_resorts_only_after_accumulated_motion_or_section_crossing() {
        let (vertices, indices) = translucent_fixture();
        let mut sorted = Translucent::new(&vertices, &indices).unwrap();
        assert!(sorted.sort([15.5, 0., 0.]));
        assert!(!sorted.sort([15.5, 0., 0.]));
        assert!(!sorted.sort([15.9, 0., 0.]));
        assert!(
            sorted.sort([16.1, 0., 0.]),
            "sub-block section crossing must sort"
        );
        assert!(!sorted.sort([16.8, 0., 0.]));
        assert!(
            sorted.sort([17.2, 0., 0.]),
            "motion accumulates from the last sort"
        );
        assert!(sorted.sort([-0.1, 0., 0.]));
        assert!(
            sorted.sort([0.1, 0., 0.]),
            "negative section crossing uses floor"
        );
        assert!(!Translucent::default().sort([0.; 3]));
    }
}
