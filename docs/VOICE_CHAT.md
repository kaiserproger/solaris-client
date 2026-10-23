# Proximity voice chat (client)

Server contract: `docs/decisions/0019-voice-chat.md` in the Solaris server repo (channel `solaris:voice`).

## Player controls

Options > Audio: **Voice chat** (off by default; microphone opens only after the server announces voice support),
**Mute microphone**, **Deafen**, **Voice volume**, **Microphone volume**. Hold **V** to talk
(push-to-talk; only while the game screen is focused, without chat or another modal).
Entering desktop control or changing input ownership releases push-to-talk.
The HUD shows `MIC` (idle), `TALK`, `MUTED`, `DEAF` or `NO MIC` once the server reports voice enabled.
Servers without voice support receive only optional channel registration when the
setting is enabled; no microphone is opened and no voice audio is sent.

## Pipeline

- Capture (`src/voice.rs`): cpal default input device (ALSA/Pulse/PipeWire on Linux, WASAPI on Windows) on a
  dedicated thread, downmixed to mono and linearly resampled to 48 kHz, cut into 20 ms frames, Opus-encoded
  (`unsafe-libopus`, pure Rust, 24 kbps VoIP) only while push-to-talk is down. Packets are pumped to the
  session once per frame (`src/voice/app.rs`).
- Handshake: the client registers `solaris:voice`, sends `LISTEN(1)` after `VOICE_STATE(enabled)` and while voice
  is on and not deafened, `LISTEN(0)` otherwise. It sends audio only while the server reports voice enabled.
- Playback (`src/audio/voicemix.rs`): per-speaker Opus decode with packet-loss concealment (up to 3 frames), a
  60 ms jitter buffer (prefill 3 frames, cap 160 ms, re-prefill after underrun), linear distance attenuation to the
  server range (no panning), mixed into the ordinary output device via one mono 48 kHz source scaled by master
  and Voice category volume.

## Test hooks (environment)

- `SOLARIS_VOICE_MIC_WAV=<file.wav>`: loop a 16-bit PCM WAV as the microphone in real time (no input device).
- `SOLARIS_VOICE_PTT_ALWAYS=1`: hold push-to-talk down.
- `SOLARIS_VOICE_RECORD=<file.wav>`: write every decoded remote frame (pre-attenuation) to a mono 48 kHz WAV.

Recorded evidence (Xvfb, private server): `/home/kaiserroman/solaris-targets/VoiceClient/e2e/received.wav`.
