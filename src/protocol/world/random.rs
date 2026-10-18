use crate::protocol::{Result, models::JavaRandom};
use parking_lot::Mutex;
use std::sync::OnceLock;

#[derive(Default)]
pub(super) struct RandomSources {
    sound: OnceLock<Result<Mutex<JavaRandom>>>,
    level: OnceLock<Result<Mutex<JavaRandom>>>,
}
impl RandomSources {
    pub(super) fn initialize(&self) -> Result<()> {
        source(&self.sound)?;
        source(&self.level)?;
        Ok(())
    }
    pub(super) fn sound_seed(&self) -> Result<u64> {
        Ok(source(&self.sound)?.lock().next_long() as u64)
    }
    pub(super) fn local_sound_seed(&self) -> Result<u64> {
        Ok(source(&self.level)?.lock().next_long() as u64)
    }
    pub(super) fn next_float(&self) -> Result<f32> {
        Ok(source(&self.level)?.lock().next_float())
    }
    pub(super) fn next_int_bound(&self, bound: u32) -> Result<u32> {
        if bound == 0 || bound > i32::MAX as u32 {
            return Err("world random bound must be a positive Java int".into());
        }
        Ok(source(&self.level)?.lock().next_int(bound))
    }
    pub(super) fn next_double(&self) -> Result<f64> {
        Ok(source(&self.level)?.lock().next_double())
    }
}
fn source(holder: &OnceLock<Result<Mutex<JavaRandom>>>) -> Result<&Mutex<JavaRandom>> {
    holder
        .get_or_init(|| {
            let mut seed = [0; 8];
            getrandom::fill(&mut seed)
                .map_err(|error| format!("world random seed entropy: {error}"))?;
            Ok(Mutex::new(JavaRandom::new(i64::from_ne_bytes(seed))))
        })
        .as_ref()
        .map_err(Clone::clone)
}
