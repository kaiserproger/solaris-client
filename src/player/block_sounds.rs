//! Local block break/step audio over the existing use-prediction sound queue.
//! Source pin: verified 26.1.2 client archive, ignored
//! `.analysis/player-research/MultiPlayerGameMode.javap` (`continueDestroyBlock`
//! hit sound every fourth destroy tick, `(volume + 1) / 8`, `pitch * 0.5`) and
//! `.analysis/player-research/Entity.javap` (`moveDist`/`nextStep` accumulation,
//! `playStepSound` gain `volume * 0.15`).
//! Place sounds already live in `use_on/placement.rs` and are untouched here;
//! session code consumes the enqueued `Sound` + `LevelEvent` predictions.
//! All event names and gains come from `Content::block_sound` /
//! `Content::block_use_sound` Source metadata; nothing is guessed.

use super::*;
use crate::audio::{SoundCategory, SoundRequest};

/// Verified SoundType gains; see `audio::materials::BlockSounds::request`.
fn hit_gain(volume: f32, pitch: f32) -> (f32, f32) {
    ((volume + 1.) / 8., pitch * 0.5)
}
fn break_gain(volume: f32, pitch: f32) -> (f32, f32) {
    ((volume + 1.) / 2., pitch * 0.8)
}
fn step_gain(volume: f32, pitch: f32) -> (f32, f32) {
    (volume * 0.15, pitch)
}

fn block_center(position: [i32; 3]) -> [f64; 3] {
    position.map(|value| f64::from(value) + 0.5)
}

impl Player {
    /// Predicted completion thump plus the texture-fragment burst trigger.
    /// Call only on actual completed positive-hardness/instant-allowed local
    /// breaks; the `Breaking::finished` guard keeps held input from repeating it.
    pub(super) fn emit_break_completion(
        &mut self,
        world: &World,
        content: &Content,
        hit: &BlockHit,
    ) -> Result<()> {
        let sound = content.block_sound(hit.state)?;
        let (volume, pitch) = break_gain(sound.volume, sound.pitch);
        self.predict_use(UsePrediction::Sound(SoundRequest::positional(
            content.block_use_sound(hit.state, "break")?,
            SoundCategory::Blocks,
            block_center(hit.position),
            volume,
            pitch,
            world.sound_seed()?,
        )))?;
        self.predict_use(UsePrediction::LevelEvent {
            event: 2001,
            position: hit.position,
            data: hit.state as i32,
        })
    }

    /// Vanilla every-fourth-destroy-tick mining knock.
    pub(super) fn emit_break_hit(
        &mut self,
        world: &World,
        content: &Content,
        hit: &BlockHit,
    ) -> Result<()> {
        let sound = content.block_sound(hit.state)?;
        let (volume, pitch) = hit_gain(sound.volume, sound.pitch);
        self.predict_use(UsePrediction::Sound(SoundRequest::positional(
            content.block_use_sound(hit.state, "hit")?,
            SoundCategory::Blocks,
            block_center(hit.position),
            volume,
            pitch,
            world.sound_seed()?,
        )))
    }

    /// Vanilla `Entity` moveDist/nextStep cadence driven by actual horizontal
    /// travel, not frame count: silent while idle, airborne, swimming, flying,
    /// gliding, riding or spectating.
    pub(super) fn accumulate_steps(
        &mut self,
        world: &World,
        content: &Content,
        from: [f64; 3],
    ) -> Result<()> {
        if !self.grounded
            || self.vehicle.is_some()
            || self.abilities.flying
            || self.swimming
            || self.gliding
            || self.game_mode == 3
        {
            return Ok(());
        }
        let dx = self.position[0] - from[0];
        let dz = self.position[2] - from[2];
        let horizontal = (dx * dx + dz * dz).sqrt();
        if horizontal <= 0. {
            return Ok(());
        }
        self.step_distance += (horizontal * 0.6) as f32;
        if self.step_distance <= self.next_step {
            return Ok(());
        }
        let below = [
            self.position[0].floor() as i32,
            (self.position[1] - 0.2).floor() as i32,
            self.position[2].floor() as i32,
        ];
        let Some(state) = world.block(below) else {
            return Ok(());
        };
        if content.info(state)?.is_air {
            return Ok(());
        }
        let sound = content.block_sound(state)?;
        let (volume, pitch) = step_gain(sound.volume, sound.pitch);
        self.predict_use(UsePrediction::Sound(SoundRequest::positional(
            content.block_use_sound(state, "step")?,
            SoundCategory::Players,
            self.position,
            volume,
            pitch,
            world.local_sound_seed()?,
        )))?;
        self.next_step = self.step_distance.floor() + 1.;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};

    fn sound_content() -> Content {
        let mut content = Content::test_cube();
        content.insert_test_block_sound(
            1,
            0.5,
            1.5,
            BTreeMap::from([
                ("hit".into(), "test:hit".into()),
                ("break".into(), "test:break".into()),
                ("place".into(), "test:place".into()),
                ("step".into(), "test:step".into()),
                ("fall".into(), "test:fall".into()),
            ]),
        );
        content
    }

    fn step_world() -> World {
        let mut world = World::new(0, 16);
        let palette = |size| Palette {
            bits: 0,
            values: vec![0],
            words: vec![],
            size,
        };
        world
            .insert(
                [0, 0],
                Chunk {
                    sections: vec![Arc::new(Section {
                        blocks: palette(4096),
                        biomes: palette(64),
                        non_empty_block_count: 0,
                    })],
                    sky: BTreeMap::new(),
                    block: BTreeMap::new(),
                    block_entities: Arc::new(vec![]),
                    surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        for x in 0..16 {
            world.set([x, 0, 0], 1, |state| Ok(state == 0)).unwrap();
        }
        world
    }

    fn break_hit(state: u32) -> BlockHit {
        BlockHit {
            position: [4, 5, 6],
            face: 1,
            point: [4.5, 5.5, 6.5],
            state,
            distance: 3.,
            inside: false,
        }
    }

    fn break_predictions(player: &mut Player) -> Vec<UsePrediction> {
        let mut out = Vec::new();
        while let Some(prediction) = player.take_use_prediction() {
            out.push(prediction);
        }
        out
    }

    #[test]
    fn break_and_hit_effects_use_source_events_and_gains() {
        let content = sound_content();
        let world = step_world();
        let hit = break_hit(1);
        let mut player = Player::new();
        player
            .emit_break_completion(&world, &content, &hit)
            .unwrap();
        let mut predictions = break_predictions(&mut player);
        assert_eq!(predictions.len(), 2);
        match predictions.remove(0) {
            UsePrediction::Sound(request) => {
                assert_eq!(request.event, "test:break");
                assert_eq!(request.category, SoundCategory::Blocks);
                assert_eq!(request.volume, (0.5 + 1.) / 2.);
                assert_eq!(request.pitch, 1.5 * 0.8);
            }
            other => panic!("expected mapped break sound, got {other:?}"),
        }
        match predictions.remove(0) {
            UsePrediction::LevelEvent {
                event,
                position,
                data,
            } => {
                assert_eq!(event, 2001);
                assert_eq!(position, hit.position);
                assert_eq!(data, hit.state as i32);
            }
            other => panic!("expected fragment burst event, got {other:?}"),
        }
        // Every fourth destroy tick knocks with verified hit gains.
        player.emit_break_hit(&world, &content, &hit).unwrap();
        match player.take_use_prediction() {
            Some(UsePrediction::Sound(request)) => {
                assert_eq!(request.event, "test:hit");
                assert_eq!(request.category, SoundCategory::Blocks);
                assert_eq!(request.volume, (0.5 + 1.) / 8.);
                assert_eq!(request.pitch, 1.5 * 0.5);
            }
            other => panic!("expected mapped hit sound, got {other:?}"),
        }
        assert!(player.take_use_prediction().is_none());
    }

    #[test]
    fn grounded_travel_plays_mapped_step_each_threshold_without_idle_or_air_duplicates() {
        let content = sound_content();
        let world = step_world();
        let mut player = Player::new();
        player.grounded = true;
        player.position = [0.5, 1.0, 0.5];
        // Idle: no travel means no step.
        player
            .accumulate_steps(&world, &content, player.position)
            .unwrap();
        assert!(player.take_use_prediction().is_none());
        // First block of travel stays below the armed threshold.
        let from = player.position;
        player.position[0] += 1.0;
        player.accumulate_steps(&world, &content, from).unwrap();
        assert!(player.take_use_prediction().is_none());
        // Crossing the threshold plays the mapped step with verified gains.
        let from = player.position;
        player.position[0] += 1.0;
        player.accumulate_steps(&world, &content, from).unwrap();
        match player.take_use_prediction() {
            Some(UsePrediction::Sound(request)) => {
                assert_eq!(request.event, "test:step");
                assert_eq!(request.category, SoundCategory::Players);
                assert_eq!(request.volume, 0.5 * 0.15);
                assert_eq!(request.pitch, 1.5);
            }
            other => panic!("expected mapped step sound, got {other:?}"),
        }
        assert!(player.take_use_prediction().is_none());
        // Airborne travel accumulates nothing and stays silent.
        player.grounded = false;
        let from = player.position;
        player.position[0] += 5.0;
        player.accumulate_steps(&world, &content, from).unwrap();
        assert!(player.take_use_prediction().is_none());
        // Re-armed threshold keeps the next short walk silent.
        player.grounded = true;
        let from = player.position;
        player.position[0] += 0.5;
        player.accumulate_steps(&world, &content, from).unwrap();
        assert!(player.take_use_prediction().is_none());
    }
}
