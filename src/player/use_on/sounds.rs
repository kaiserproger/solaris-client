use super::*;
use crate::audio::{SoundCategory, SoundOrigin, SoundRequest};

impl Player {
    pub(super) fn play_use_level_sound(
        &mut self,
        world: &World,
        event: &str,
        category: SoundCategory,
        position: [f64; 3],
        volume: f32,
        pitch: f32,
        except_local_player: bool,
    ) -> Result<()> {
        let seed = world.sound_seed()?;
        if except_local_player {
            self.predict_use(UsePrediction::Sound(SoundRequest::positional(
                event, category, position, volume, pitch, seed,
            )))?;
        }
        Ok(())
    }

    pub(super) fn play_use_entity_sound(
        &mut self,
        world: &World,
        event: &str,
        category: SoundCategory,
        handle: Handle,
        position: [f64; 3],
        volume: f32,
        pitch: f32,
        except_local_player: bool,
    ) -> Result<()> {
        let seed = world.sound_seed()?;
        if except_local_player {
            let mut request =
                SoundRequest::positional(event, category, position, volume, pitch, seed);
            request.source = SoundOrigin::Entity {
                id: handle.network_id,
                generation: handle.generation,
                position,
            };
            self.predict_use(UsePrediction::Sound(request))?;
        }
        Ok(())
    }

    pub(super) fn play_use_local_sound(
        &mut self,
        world: &World,
        event: &str,
        volume: f32,
        pitch: f32,
    ) -> Result<()> {
        self.predict_use(UsePrediction::Sound(SoundRequest::positional(
            event,
            SoundCategory::Players,
            self.position,
            volume,
            pitch,
            world.local_sound_seed()?,
        )))
    }
}
