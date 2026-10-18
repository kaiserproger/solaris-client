//! Source point/dimension attribute evaluation. Unlike the camera probe, a block
//! query selects one seed-zoomed biome and never spatially averages booleans.
//! Provenance: retained 26.1.2 EnvironmentAttributeSystem, BooleanModifier,
//! BiomeManager, Level and WeatherAttributes bytecode under ignored .analysis.
use super::{
    Environment,
    attributes::{Attribute as A, AttributeMap, COUNT, Datum, Track},
};
use crate::protocol::{Result, content::Content, world::World};
use std::sync::Arc;

pub(crate) struct PointRules {
    dimension: AttributeMap,
    biomes: Arc<Vec<AttributeMap>>,
    timelines: Vec<(String, Option<i64>, Vec<Track>)>,
    biome_boolean: [bool; COUNT],
    pub(crate) has_skylight: bool,
    weather: bool,
}
impl PointRules {
    pub(super) fn new(environment: &Environment, has_skylight: bool) -> Result<Self> {
        let dimension = environment
            .dimension
            .as_ref()
            .ok_or("point environment dimension missing")?;
        Ok(Self {
            dimension: dimension.attributes,
            biomes: environment.biomes.clone(),
            timelines: dimension
                .timeline_ids
                .iter()
                .map(|id| {
                    let timeline = &environment.timelines[*id].1;
                    (
                        timeline.clock.clone(),
                        timeline.period,
                        timeline
                            .tracks
                            .iter()
                            .filter(|track| {
                                track.attribute.boolean() || track.attribute == A::SkyLightLevel
                            })
                            .cloned()
                            .collect(),
                    )
                })
                .collect(),
            biome_boolean: std::array::from_fn(|i| {
                environment.biomes.iter().any(|map| map[i].is_some())
            }),
            has_skylight,
            weather: dimension.weather,
        })
    }
}
#[derive(Default)]
pub(crate) struct PointState {
    rules: Option<Arc<PointRules>>,
    times: Vec<i64>,
    rain: f32,
    thunder: f32,
}
impl PointState {
    pub(super) fn publish(&mut self, environment: &Environment) -> Result<()> {
        let rules = environment
            .point_rules
            .as_ref()
            .ok_or("point environment not activated")?;
        if !self
            .rules
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, rules))
        {
            self.times.resize(rules.timelines.len(), 0);
            self.rules = Some(rules.clone());
        }
        for (time, (clock, _, _)) in self.times.iter_mut().zip(&rules.timelines) {
            *time = environment
                .clocks
                .get(clock)
                .ok_or("point timeline clock missing")?
                .ticks;
        }
        self.rain = environment.rain;
        self.thunder = environment.thunder;
        Ok(())
    }
    fn rules(&self) -> Result<&PointRules> {
        self.rules
            .as_deref()
            .ok_or("world point environment unpublished".into())
    }
    pub(crate) fn has_skylight(&self) -> Result<bool> {
        Ok(self.rules()?.has_skylight)
    }
    fn base(&self, attribute: A) -> Result<Datum> {
        let rules = self.rules()?;
        let base = attribute.default_value();
        Ok(rules.dimension[attribute as usize].map_or(base, |modifier| modifier.apply(base)))
    }
    fn finish(&self, attribute: A, mut value: Datum) -> Result<Datum> {
        let rules = self.rules()?;
        for ((_, period, tracks), time) in rules.timelines.iter().zip(&self.times) {
            for track in tracks {
                if track.attribute == attribute {
                    value = track.apply(value, *time, *period)?;
                }
            }
        }
        Ok(value)
    }
    fn point_boolean(
        &self,
        attribute: A,
        world: &World,
        position: [i32; 3],
        content: &Content,
    ) -> Result<bool> {
        let rules = self.rules()?;
        let mut value = self.base(attribute)?;
        if attribute.positional() && rules.biome_boolean[attribute as usize] {
            let biome = world.source_biome(position)?;
            content.registry_entry("minecraft:worldgen/biome", biome as i32)?;
            let map = rules
                .biomes
                .get(biome as usize)
                .ok_or("point biome attribute holder missing")?;
            if let Some(modifier) = map[attribute as usize] {
                value = modifier.apply(value);
            }
        }
        Ok(self.finish(attribute, value)?.boolean())
    }
    pub(crate) fn water_evaporates(
        &self,
        world: &World,
        position: [i32; 3],
        content: &Content,
    ) -> Result<bool> {
        self.point_boolean(A::WaterEvaporates, world, position, content)
    }
    pub(crate) fn creaking_active(
        &self,
        world: &World,
        position: [i32; 3],
        content: &Content,
    ) -> Result<bool> {
        self.point_boolean(A::CreakingActive, world, position, content)
    }
    pub(crate) fn fast_lava(&self) -> Result<bool> {
        Ok(self.finish(A::FastLava, self.base(A::FastLava)?)?.boolean())
    }
    pub(crate) fn sky_darken(&self) -> Result<u8> {
        let mut light = self
            .finish(A::SkyLightLevel, self.base(A::SkyLightLevel)?)?
            .float();
        if self.rules()?.weather {
            let thunder = self.thunder * self.rain;
            // Weather layer applies rain-without-thunder first, then thunder.
            for (strength, alpha) in [(self.rain - thunder, 0.3125), (thunder, 0.52734375)] {
                if strength > 0. {
                    let modified = light + alpha * (4. - light);
                    light += strength * (modified - light);
                }
            }
        }
        Ok((15. - light.clamp(0., 15.)) as u8)
    }
    pub(crate) fn raining(&self) -> Result<bool> {
        Ok(self.rules()?.weather && self.rain > 0.2)
    }
    pub(crate) fn thundering(&self) -> Result<bool> {
        Ok(self.rules()?.weather && self.thunder * self.rain > 0.9)
    }
}
