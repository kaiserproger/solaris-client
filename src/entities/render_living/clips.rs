use super::super::resources::Rig;
use crate::protocol::Result;
use glam::Vec3;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Debug, Deserialize)]
#[serde(from = "CapturedDefinition")]
pub(crate) struct AnimationDefinition {
    pub length: f32,
    pub looping: bool,
    bones: BTreeMap<String, Range<usize>>,
    channels: Vec<AnimationChannel>,
}
#[derive(Deserialize)]
struct CapturedDefinition {
    length: f32,
    looping: bool,
    bones: BTreeMap<String, Vec<AnimationChannel>>,
}
impl From<CapturedDefinition> for AnimationDefinition {
    fn from(captured: CapturedDefinition) -> Self {
        let count = captured.bones.values().map(Vec::len).sum();
        let mut channels = Vec::with_capacity(count);
        let bones = captured
            .bones
            .into_iter()
            .map(|(name, values)| {
                let start = channels.len();
                channels.extend(values);
                (name, start..channels.len())
            })
            .collect();
        Self {
            length: captured.length,
            looping: captured.looping,
            bones,
            channels,
        }
    }
}
#[derive(Debug, Deserialize)]
pub(crate) struct AnimationChannel {
    pub target: Target,
    pub keys: Vec<Keyframe>,
}
#[derive(Debug, Deserialize)]
pub(crate) struct Keyframe {
    pub time: f32,
    pub pre: [f32; 3],
    pub post: [f32; 3],
    pub interpolation: Interpolation,
}
#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Target {
    Position,
    Rotation,
    Scale,
}
#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Interpolation {
    Linear,
    Catmullrom,
}
#[derive(Clone, Copy)]
pub(super) struct Pose {
    pub pivot: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub visible: bool,
}
impl Pose {
    pub fn reset(rig: &Rig, poses: &mut Vec<Self>) {
        poses.clear();
        poses.extend(rig.bones.iter().map(|bone| Self {
            pivot: bone.pivot,
            rotation: bone.rotation,
            scale: bone.scale,
            visible: true,
        }));
    }
}
struct BoundChannel {
    bone: usize,
    channel: usize,
}
pub(super) struct BoundClip {
    channels: Vec<BoundChannel>,
}
impl AnimationDefinition {
    pub fn validate(&self) -> Result<()> {
        if !self.length.is_finite() || self.length < 0. || self.bones.len() > 256 {
            return Err("source animation definition bounds".into());
        }
        let mut count = 0;
        for channel in &self.channels {
            if channel.keys.is_empty() || channel.keys.len() > 8192 {
                return Err("source animation keyframe bounds".into());
            }
            let mut previous = -1.;
            for key in &channel.keys {
                if !key.time.is_finite()
                    || key.time < previous
                    || key.time < 0.
                    || key
                        .pre
                        .iter()
                        .chain(&key.post)
                        .any(|value| !value.is_finite())
                {
                    return Err("source animation keyframe invalid".into());
                }
                previous = key.time;
            }
            count += channel.keys.len();
        }
        if count > 65536 {
            return Err("source animation aggregate keyframe bound".into());
        }
        Ok(())
    }
    pub(super) fn bind(&self, rig: &Rig) -> Result<BoundClip> {
        self.validate()?;
        let mut bindings = Vec::with_capacity(self.channels.len());
        for (name, channels) in &self.bones {
            let index = rig
                .part_by_name
                .get(name)
                .copied()
                .ok_or_else(|| format!("source animation bone not baked: {name}"))?;
            for channel in channels.clone() {
                bindings.push(BoundChannel {
                    bone: index,
                    channel,
                });
            }
        }
        Ok(BoundClip { channels: bindings })
    }
}
impl BoundClip {
    pub(super) fn apply(
        &self,
        definition: &AnimationDefinition,
        elapsed_millis: i64,
        weight: f32,
        poses: &mut [Pose],
    ) {
        let mut seconds = elapsed_millis as f32 / 1000.;
        if definition.looping {
            seconds %= definition.length;
        }
        for binding in &self.channels {
            let channel = &definition.channels[binding.channel];
            let value = sample(channel, seconds, weight);
            let pose = &mut poses[binding.bone];
            match channel.target {
                Target::Position => pose.pivot += value / 16.,
                Target::Rotation => pose.rotation += value,
                Target::Scale => pose.scale += value,
            }
        }
    }
}
pub(crate) fn sample(channel: &AnimationChannel, seconds: f32, weight: f32) -> Vec3 {
    let keys = &channel.keys;
    // Source binarySearch is a lower bound on timestamp >= time. Exact timestamps
    // use the incoming pre-target; the next sample uses that key's post-target.
    let next_at = keys.partition_point(|key| !(seconds <= key.time));
    let previous = next_at.saturating_sub(1);
    let next = (previous + 1).min(keys.len() - 1);
    let a = &keys[previous];
    let b = &keys[next];
    let fraction = if previous == next {
        0.
    } else {
        ((seconds - a.time) / (b.time - a.time)).clamp(0., 1.)
    };
    match b.interpolation {
        Interpolation::Linear => {
            let from = Vec3::from_array(a.post);
            (from + (Vec3::from_array(b.pre) - from) * fraction) * weight
        }
        Interpolation::Catmullrom => {
            let p0 = keys[previous.saturating_sub(1)].post;
            let p1 = a.post;
            let p2 = b.post;
            let p3 = keys[(next + 1).min(keys.len() - 1)].post;
            Vec3::from_array(std::array::from_fn(|i| {
                catmull(fraction, p0[i], p1[i], p2[i], p3[i]) * weight
            }))
        }
    }
}
fn catmull(t: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2. * p1
        + (p2 - p0) * t
        + (2. * p0 - 5. * p1 + 4. * p2 - p3) * t * t
        + (3. * p1 - p0 - 3. * p2 + p3) * t * t * t)
}
