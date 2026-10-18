//! Client-only multipart geometry from the received dragon transform and its 64-tick
//! flight history. No server phase steering, damage, pathfinder or AI is simulated.
use super::{Entity, Models, physical};
use crate::{
    player::{Bounds, math},
    protocol::Result,
};
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Sample {
    pub y: f64,
    pub yaw: f32,
}
#[derive(Debug)]
pub(crate) struct Flight {
    samples: [Sample; 64],
    head: Option<usize>,
    pub parts: [[f64; 3]; 8],
}
impl Default for Flight {
    fn default() -> Self {
        Self {
            samples: [Sample::default(); 64],
            head: None,
            parts: [[0.; 3]; 8],
        }
    }
}
impl Flight {
    pub fn sample(&self, delay: usize) -> Sample {
        self.samples[self.head.unwrap_or(63).wrapping_sub(delay) & 63]
    }
    pub fn record(&mut self, y: f64, yaw: f32) {
        let sample = Sample { y, yaw };
        if self.head.is_none() {
            self.samples.fill(sample);
        }
        let head = self.head.map_or(0, |h| (h + 1) & 63);
        self.head = Some(head);
        self.samples[head] = sample;
    }
    pub fn update(&mut self, position: [f64; 3], yaw: f32, sitting: bool) {
        self.record(position[1], yaw);
        let pitch = (self.sample(5).y - self.sample(10).y) as f32 * 10. * 0.017453292;
        let pitch_cos = math::cos(f64::from(pitch));
        let pitch_sin = math::sin(f64::from(pitch));
        let radians = yaw * 0.017453292;
        let sin = math::sin(f64::from(radians));
        let cos = math::cos(f64::from(radians));
        let y = if sitting {
            -1.
        } else {
            (self.sample(5).y - self.sample(0).y) as f32
        };
        let mut offsets = [[0.; 3]; 8];
        offsets[0] = [
            f64::from(sin * 6.5 * pitch_cos),
            f64::from(y + pitch_sin * 6.5),
            f64::from(-cos * 6.5 * pitch_cos),
        ];
        offsets[1] = [
            f64::from(sin * 5.5 * pitch_cos),
            f64::from(y + pitch_sin * 5.5),
            f64::from(-cos * 5.5 * pitch_cos),
        ];
        offsets[2] = [f64::from(sin * 0.5), 0., f64::from(-cos * 0.5)];
        offsets[6] = [f64::from(cos * 4.5), 2., f64::from(sin * 4.5)];
        offsets[7] = [f64::from(cos * -4.5), 2., f64::from(sin * -4.5)];
        let base = self.sample(5);
        for i in 0..3 {
            let sample = self.sample(12 + i * 2);
            let wrapped = (sample.yaw - base.yaw + 180.).rem_euclid(360.) - 180.;
            let angle = radians + wrapped * 0.017453292;
            let tail_sin = math::sin(f64::from(angle));
            let tail_cos = math::cos(f64::from(angle));
            let length = (i + 1) as f32 * 2.;
            offsets[3 + i] = [
                f64::from(-(sin * 1.5 + tail_sin * length) * pitch_cos),
                sample.y - base.y - f64::from((length + 1.5) * pitch_sin) + 1.5,
                f64::from((cos * 1.5 + tail_cos * length) * pitch_cos),
            ];
        }
        self.parts = offsets.map(|offset| std::array::from_fn(|i| position[i] + offset[i]));
    }
    pub fn bounds(&self, index: usize) -> Result<Bounds> {
        let (width, height) = match index {
            0 => (1., 1.),
            1 => (3., 3.),
            2 => (5., 3.),
            3..=5 => (2., 2.),
            6 | 7 => (4., 2.),
            _ => return Err("invalid dragon child index".into()),
        };
        let p = self.parts[index];
        Ok(Bounds {
            min: [p[0] - width / 2., p[1], p[2] - width / 2.],
            max: [p[0] + width / 2., p[1] + height, p[2] + width / 2.],
        })
    }
}
pub(crate) fn update(models: &Models, e: &mut Entity, tick: u64) {
    let transform = e.presented_transform(tick, 0.);
    let phase = physical::number(models, e, "DATA_PHASE", 0);
    let flight = e
        .physical
        .dragon
        .get_or_insert_with(|| Box::new(Flight::default()));
    flight.update(transform.position, transform.yaw, matches!(phase, 5..=7));
}
