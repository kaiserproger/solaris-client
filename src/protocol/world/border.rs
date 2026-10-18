//! Pinned 26.1.2 WorldBorder/MovingBorderExtent, not legacy wall-clock interpolation.
//! Source: ignored evidence/C04/world-heightmap-border-source.javap and receiver dump.
#[derive(Clone)]
pub struct Border {
    pub center: [f64; 2],
    pub absolute_max_size: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
    extent: Extent,
}
#[derive(Clone)]
enum Extent {
    Stationary(f64),
    Moving {
        from: f64,
        to: f64,
        duration: i64,
        remaining: i64,
        size: f64,
        previous: f64,
    },
}
impl Default for Border {
    fn default() -> Self {
        Self {
            center: [0.; 2],
            absolute_max_size: 29_999_984,
            warning_blocks: 5,
            warning_time: 15,
            extent: Extent::Stationary(59_999_968.),
        }
    }
}
impl Border {
    pub fn set_size(&mut self, size: f64) {
        self.extent = Extent::Stationary(size);
    }
    pub fn lerp_size(&mut self, from: f64, to: f64, ticks: i64) {
        self.extent = if from == to {
            Extent::Stationary(to)
        } else {
            let size = calculate_size(from, to, ticks, ticks);
            Extent::Moving {
                from,
                to,
                duration: ticks,
                remaining: ticks,
                size,
                previous: size,
            }
        };
    }
    /// Exactly one update per client-world tick; render sampling never advances state.
    pub fn tick(&mut self) {
        if let Extent::Moving {
            from,
            to,
            duration,
            remaining,
            size,
            previous,
        } = &mut self.extent
        {
            *remaining = remaining.wrapping_sub(1);
            *previous = *size;
            *size = calculate_size(*from, *to, *duration, *remaining);
            if *remaining <= 0 {
                self.extent = Extent::Stationary(*to);
            }
        }
    }
    pub fn size(&self) -> f64 {
        match self.extent {
            Extent::Stationary(size) | Extent::Moving { size, .. } => size,
        }
    }
    pub fn target_size(&self) -> f64 {
        match self.extent {
            Extent::Stationary(size) => size,
            Extent::Moving { to, .. } => to,
        }
    }
    pub fn remaining_ticks(&self) -> i64 {
        match self.extent {
            Extent::Stationary(_) => 0,
            Extent::Moving { remaining, .. } => remaining,
        }
    }
    pub fn lerp_speed(&self) -> f64 {
        match self.extent {
            Extent::Stationary(_) => 0.,
            Extent::Moving {
                from, to, duration, ..
            } => (from - to).abs() / duration as f64,
        }
    }
    pub fn bounds(&self, partial: f32) -> [f64; 4] {
        let size = match self.extent {
            Extent::Stationary(size) => size,
            Extent::Moving { previous, size, .. } => {
                previous + f64::from(partial) * (size - previous)
            }
        };
        let half = size / 2.;
        let max = f64::from(self.absolute_max_size);
        [
            clamp(self.center[0] - half, -max, max),
            clamp(self.center[1] - half, -max, max),
            clamp(self.center[0] + half, -max, max),
            clamp(self.center[1] + half, -max, max),
        ]
    }
    pub fn contains(&self, position: [f64; 3]) -> bool {
        // Source no-partial bounds sample 0, hence the previous tick's moving size.
        let [min_x, min_z, max_x, max_z] = self.bounds(0.);
        position[0] >= min_x && position[0] < max_x && position[2] >= min_z && position[2] < max_z
    }
}
fn calculate_size(from: f64, to: f64, duration: i64, remaining: i64) -> f64 {
    let progress = (duration as f64 - remaining as f64) / duration as f64;
    if progress < 1. {
        from + progress * (to - from)
    } else {
        to
    }
}
fn clamp(value: f64, min: f64, max: f64) -> f64 {
    // Mth.clamp's dcmpg selects the lower bound for NaN, unlike Rust f64::min.
    if value < min || value.is_nan() {
        min
    } else {
        value.min(max)
    }
}
