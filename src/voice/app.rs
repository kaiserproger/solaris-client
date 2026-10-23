//! Native application side of voice chat: setting gate, push-to-talk key, packet pump.
use super::{Capture, MicState, audio_packet, listen_packet};
use crate::App;
use winit::keyboard::KeyCode;

/// Push-to-talk key (configurable key binding is not part of the first cut).
pub(crate) const PTT_KEY: KeyCode = KeyCode::KeyV;

#[derive(Default)]
pub(crate) struct Client {
    capture: Option<Capture>,
    key_down: bool,
}

/// The microphone opens only when the player enabled voice chat in play **and** the server
/// announced `VOICE_STATE(enabled)`; against a server without `solaris:voice` it never opens.
fn microphone_wanted(setting_in_play: bool, server_enabled: bool) -> bool {
    setting_in_play && server_enabled
}

impl App {
    pub(crate) fn voice_key(&mut self, code: KeyCode, pressed: bool) {
        if code == PTT_KEY {
            self.voice.key_down = pressed;
        }
    }
    pub(crate) fn voice_focus_lost(&mut self) {
        self.voice.key_down = false;
    }
    /// Once per frame: start/stop the microphone with the Options toggle and pump packets.
    pub(crate) fn update_voice(&mut self) {
        let enabled = self
            .session
            .native_setting("voice_chat")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            && self.session.phase == "play";
        if enabled {
            if let Err(error) = self.session.register_voice() {
                eprintln!("voice register failed: {error}");
            }
        }
        // The microphone is only opened once the server announced voice support: against a
        // server without `solaris:voice` the toggle stays inert and silent.
        if microphone_wanted(enabled, self.session.voice_net.enabled) {
            if self.voice.capture.is_none() {
                self.voice.capture = Some(Capture::start());
            }
        } else if self.voice.capture.take().is_some() {
            self.session.set_voice_mic(None);
        }
        let flag = |app: &Self, key: &str| app.session.native_setting(key).and_then(serde_json::Value::as_bool).unwrap_or(false);
        let level = |app: &Self, key: &str| app.session.native_setting(key).and_then(serde_json::Value::as_f64).unwrap_or(1.) as f32;
        let (muted, deaf) = (flag(self, "voice_mute"), flag(self, "voice_deafen"));
        if let Some(audio) = self.session.audio() {
            audio.voice().set_volume(level(self, "voice_volume"));
        }
        let mic_volume = level(self, "voice_mic_volume");
        let server = self.session.voice_net.enabled;
        let listen = enabled && server && !deaf;
        if listen != self.session.voice_net.listening {
            if self.session.send_voice(listen_packet(listen)).is_ok() {
                self.session.voice_net.listening = listen;
            }
        }
        let Some(capture) = &self.voice.capture else { return };
        let game_input = self.focused && self.session.screen() == "game"
            && !self.session.chat_open() && !self.session.dialog_screen_active()
            && !self.computer.has_modal();
        capture.set_gain(mic_volume);
        capture.set_ptt(self.voice.key_down && game_input && !muted);
        while let Ok((seq, packet)) = capture.packets.try_recv() {
            if listen && let Err(error) = self.session.send_voice(audio_packet(seq, &packet)) {
                eprintln!("voice send failed: {error}");
            }
        }
        let state = if server { capture.state() } else { MicState::Off };
        self.session.set_voice_mic(match state {
            MicState::Off => None,
            _ if deaf => Some(("DEAF", [0.6, 0.35, 0.05])),
            _ if muted => Some(("MUTED", [0.6, 0.35, 0.05])),
            MicState::Idle => Some(("MIC", [0.15, 0.15, 0.15])),
            MicState::Talking => Some(("TALK", [0.1, 0.55, 0.15])),
            MicState::Unavailable => Some(("NO MIC", [0.6, 0.1, 0.1])),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::microphone_wanted;

    #[test]
    fn microphone_stays_closed_without_server_voice_support() {
        assert!(!microphone_wanted(true, false), "server never offered voice");
        assert!(!microphone_wanted(false, true), "player left voice chat off");
        assert!(microphone_wanted(true, true));
    }
}
