//! Pleasant synthesized completion chimes, Pomodoro chimes, and focus ticking across Windows, macOS, and Linux.

use fern_core::Store;
use gpui::App;

use crate::active_preferences::ActivePreferences;

#[cfg(all(not(test), target_arch = "wasm32"))]
#[path = "sound/wasm.rs"]
mod wasm_audio;

#[cfg(all(windows, not(test)))]
#[path = "sound/windows.rs"]
mod platform;

#[cfg(all(target_os = "macos", not(test)))]
#[path = "sound/macos.rs"]
mod platform;

#[cfg(any(all(target_os = "linux", not(test)), test))]
#[path = "sound/linux.rs"]
mod linux;

#[cfg(all(target_os = "linux", not(test)))]
use linux as platform;

#[cfg(all(
    not(any(windows, target_os = "macos", target_os = "linux", target_arch = "wasm32")),
    not(test)
))]
mod platform {
    pub(super) struct AudioBackend;

    impl AudioBackend {
        pub(super) fn new() -> Self {
            Self
        }
        pub(super) fn play_once(&mut self, _tag: &str, _wav: &'static [u8]) {}
        pub(super) fn play_loop(&mut self, _wav: &'static [u8]) {}
        pub(super) fn stop(&mut self) {}
        pub(super) fn reap_deadline(&self) -> Option<std::time::Instant> {
            None
        }
        pub(super) fn poll_reap(&mut self, _now: std::time::Instant) {}
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
use platform::AudioBackend;

const SETTING: &str = "completion_sound";
const WHITE_NOISE_SETTING: &str = "focus_white_noise";

/// Ambient focus sound presets (pure-Rust procedural PCM, zero external assets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhiteNoise {
    #[default]
    Off,
    Ticking,
}

impl WhiteNoise {
    pub const ALL: [Self; 2] = [Self::Off, Self::Ticking];

    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Ticking => "ticking",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "ticking" => Self::Ticking,
            _ => Self::Off,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Ticking,
            Self::Ticking => Self::Off,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Ticking => 1,
        }
    }
}

pub fn white_noise(store: &Store) -> WhiteNoise {
    store
        .setting(WHITE_NOISE_SETTING)
        .ok()
        .flatten()
        .as_deref()
        .map(WhiteNoise::from_key)
        .unwrap_or_default()
}

pub fn set_white_noise(store: &Store, preset: WhiteNoise) -> anyhow::Result<()> {
    store.set_setting(WHITE_NOISE_SETTING, preset.key())
}

/// Starts or stops looping focus white noise depending on whether focus is actively running.
///
/// PCM synthesis and OS audio playback run on a dedicated background audio thread so
/// the UI thread never stalls.
pub fn sync_white_noise(active: bool, preset: WhiteNoise) {
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        let desired = if active { preset } else { WhiteNoise::Off };
        if desired == WhiteNoise::Off && AUDIO_TX.get().is_none() {
            return;
        }
        send_audio(AudioCommand::SetWhiteNoise(desired));
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        let desired = if active { preset } else { WhiteNoise::Off };
        if desired == WhiteNoise::Off {
            wasm_audio::stop_loop();
        } else {
            let wav = white_noise_wav(desired);
            if !wav.is_empty() {
                wasm_audio::play_loop(wav);
            }
        }
    }
    #[cfg(test)]
    {
        let _ = (active, preset);
    }
}

pub fn stop_white_noise() {
    sync_white_noise(false, WhiteNoise::Off);
}

/// Whether completion & focus chimes are enabled (default: on).
pub fn enabled(store: &Store) -> bool {
    store.setting(SETTING).ok().flatten().as_deref() != Some("0")
}

/// Persists completion sound preference and keeps the [`ActivePreferences`] global in sync.
pub fn set_enabled(store: &Store, on: bool, cx: &mut App) -> anyhow::Result<()> {
    store.set_setting(SETTING, if on { "1" } else { "0" })?;
    ActivePreferences::update(cx, |prefs| prefs.completion_sound = on);
    Ok(())
}

/// Plays a short, crisp chime when a task or habit is completed (reading the cached preference).
pub fn play_complete(cx: &App) {
    if ActivePreferences::get(cx).completion_sound {
        #[cfg(all(not(test), not(target_arch = "wasm32")))]
        send_audio(AudioCommand::PlayChime(Chime::Complete));
        #[cfg(all(not(test), target_arch = "wasm32"))]
        wasm_audio::play_once(complete_wav());
    }
}

/// Plays a warm three-note chime when a Pomodoro session finishes (reading the cached preference).
pub fn play_focus_done(cx: &App) {
    if ActivePreferences::get(cx).completion_sound {
        #[cfg(all(not(test), not(target_arch = "wasm32")))]
        send_audio(AudioCommand::PlayChime(Chime::FocusDone));
        #[cfg(all(not(test), target_arch = "wasm32"))]
        wasm_audio::play_once(focus_done_wav());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chime {
    Complete,
    FocusDone,
}

#[cfg(not(target_arch = "wasm32"))]
impl Chime {
    fn duration(self) -> std::time::Duration {
        // Slightly longer than the synthesized chime tail (0.21s / 0.52s) so looping ambient
        // audio resumes right after the chime finishes without clipping it.
        match self {
            Self::Complete => std::time::Duration::from_millis(260),
            Self::FocusDone => std::time::Duration::from_millis(570),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioCommand {
    SetWhiteNoise(WhiteNoise),
    PlayChime(Chime),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioEffect {
    None,
    Stop,
    LoopWhiteNoise(WhiteNoise),
    PlayChime(Chime),
}

/// Pure single-channel playback coordinator: because single-stream backends replace active
/// looping ambient audio when a chime plays, this state machine tracks the desired ambient preset
/// and schedules an automatic resume once the chime ends.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct AudioState {
    desired_ambient: WhiteNoise,
    resume_at: Option<std::time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AudioState {
    fn handle(&mut self, cmd: AudioCommand, now: std::time::Instant) -> AudioEffect {
        match cmd {
            AudioCommand::SetWhiteNoise(preset) => {
                if preset == self.desired_ambient {
                    return AudioEffect::None;
                }
                self.desired_ambient = preset;
                self.resume_at = None;
                match preset {
                    WhiteNoise::Off => AudioEffect::Stop,
                    other => AudioEffect::LoopWhiteNoise(other),
                }
            }
            AudioCommand::PlayChime(chime) => {
                if self.desired_ambient != WhiteNoise::Off {
                    self.resume_at = Some(now + chime.duration());
                }
                AudioEffect::PlayChime(chime)
            }
        }
    }

    fn on_timeout(&mut self) -> AudioEffect {
        self.resume_at = None;
        match self.desired_ambient {
            WhiteNoise::Off => AudioEffect::None,
            other => AudioEffect::LoopWhiteNoise(other),
        }
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
static AUDIO_TX: std::sync::OnceLock<std::sync::mpsc::Sender<AudioCommand>> =
    std::sync::OnceLock::new();

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn send_audio(cmd: AudioCommand) {
    let tx = AUDIO_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AudioCommand>();
        let _ = std::thread::Builder::new()
            .name("fern-audio".into())
            .spawn(move || audio_worker(rx));
        tx
    });
    let _ = tx.send(cmd);
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn apply_effect(backend: &mut AudioBackend, effect: AudioEffect) {
    match effect {
        AudioEffect::None => {}
        AudioEffect::Stop => backend.stop(),
        AudioEffect::LoopWhiteNoise(preset) => backend.play_loop(white_noise_wav(preset)),
        AudioEffect::PlayChime(Chime::Complete) => backend.play_once("complete", complete_wav()),
        AudioEffect::PlayChime(Chime::FocusDone) => backend.play_once("focus_done", focus_done_wav()),
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn audio_worker(rx: std::sync::mpsc::Receiver<AudioCommand>) {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;

    let mut state = AudioState::default();
    let mut backend = AudioBackend::new();

    loop {
        let next_deadline = match (state.resume_at, backend.reap_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(deadline) = next_deadline {
            let wait = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(cmd) => {
                    let now = Instant::now();
                    backend.poll_reap(now);
                    apply_effect(&mut backend, state.handle(cmd, now));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if state.resume_at.is_some_and(|t| now >= t) {
                        apply_effect(&mut backend, state.on_timeout());
                    }
                    backend.poll_reap(now);
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => apply_effect(&mut backend, state.handle(cmd, Instant::now())),
                Err(_) => break,
            }
        }
    }
    backend.stop();
}

/// Generates a 16-bit mono 44.1 kHz PCM WAV buffer for a sequence of `(freq_hz, start_s, dur_s)` notes.
fn synthesize_wav(notes: &[(f32, f32, f32)], total_s: f32, volume: f32) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 44_100;
    let num_samples = (SAMPLE_RATE as f32 * total_s).ceil() as usize;
    let data_bytes = (num_samples * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());

    let tau = std::f32::consts::TAU;
    for i in 0..num_samples {
        let t = i as f32 / SAMPLE_RATE as f32;
        let mut mix = 0.0f32;
        for &(freq, start, dur) in notes {
            if t >= start && t < start + dur {
                let local = t - start;
                let attack = (local / 0.008).min(1.0);
                let decay = (-local / (dur * 0.35)).exp();
                let env = attack * decay;
                let fundamental = (tau * freq * local).sin();
                let overtone = 0.25 * (tau * freq * 2.0 * local).sin();
                mix += (fundamental + overtone) * env;
            }
        }
        let sample = (mix * volume).clamp(-1.0, 1.0);
        wav.extend_from_slice(&((sample * i16::MAX as f32) as i16).to_le_bytes());
    }
    wav
}

#[cfg(not(test))]
fn complete_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| synthesize_wav(&[(783.99, 0.0, 0.11), (1046.50, 0.055, 0.15)], 0.21, 0.22))
}

#[cfg(not(test))]
fn focus_done_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    WAV.get_or_init(|| {
        synthesize_wav(&[(523.25, 0.0, 0.22), (659.25, 0.09, 0.24), (783.99, 0.18, 0.32)], 0.52, 0.24)
    })
}

/// Generates a seamless 4-second 16-bit mono 22.05 kHz PCM WAV loop for an ambient focus preset.
fn synthesize_ambient(preset: WhiteNoise) -> Vec<u8> {
    const SAMPLE_RATE: u32 = 22_050;
    const SECONDS: usize = 4;
    let num_samples = SAMPLE_RATE as usize * SECONDS;
    let data_bytes = (num_samples * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());

    let mut samples = vec![0.0f32; num_samples];

    match preset {
        WhiteNoise::Off => {}
        WhiteNoise::Ticking => {
            let sr = SAMPLE_RATE as f32;
            for (i, s) in samples.iter_mut().enumerate() {
                let sec_pos = (i % SAMPLE_RATE as usize) as f32 / sr;
                if sec_pos < 0.03 {
                    let freq = if (i / SAMPLE_RATE as usize).is_multiple_of(2) { 920.0 } else { 760.0 };
                    let env = (-sec_pos / 0.005).exp();
                    *s = (std::f32::consts::TAU * freq * sec_pos).sin() * env * 0.14;
                }
            }
        }
    }

    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        wav.extend_from_slice(&((clamped * i16::MAX as f32) as i16).to_le_bytes());
    }
    wav
}

#[cfg(not(test))]
fn white_noise_wav(preset: WhiteNoise) -> &'static [u8] {
    use std::sync::OnceLock;
    static TICKING: OnceLock<Vec<u8>> = OnceLock::new();
    match preset {
        WhiteNoise::Ticking => TICKING.get_or_init(|| synthesize_ambient(WhiteNoise::Ticking)),
        WhiteNoise::Off => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn completion_sound_defaults_on_and_persists(cx: &mut gpui::TestAppContext) {
        let store = Store::open_in_memory().unwrap();
        assert!(enabled(&store));
        cx.update(|cx| {
            set_enabled(&store, false, cx).unwrap();
            assert!(!enabled(&store));
            assert!(!ActivePreferences::get(cx).completion_sound);
            set_enabled(&store, true, cx).unwrap();
            assert!(enabled(&store));
            assert!(ActivePreferences::get(cx).completion_sound);
        });
    }

    #[test]
    fn white_noise_defaults_off_and_persists() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Off);
        set_white_noise(&store, WhiteNoise::Ticking).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Ticking);
        set_white_noise(&store, WhiteNoise::Off).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Off);

        assert_eq!(WhiteNoise::from_key("rain"), WhiteNoise::Off);
        assert_eq!(WhiteNoise::from_key("ocean"), WhiteNoise::Off);
    }

    #[test]
    fn synthesized_wav_has_valid_riff_header() {
        let wav = synthesize_wav(&[(784.0, 0.0, 0.1)], 0.12, 0.2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert!(wav.len() > 44);
        let amb = synthesize_ambient(WhiteNoise::Ticking);
        assert_eq!(&amb[0..4], b"RIFF");
        assert_eq!(&amb[8..12], b"WAVE");
        assert!(amb.len() > 44);
    }

    #[test]
    fn chime_during_white_noise_schedules_and_resumes_loop() {
        let now = std::time::Instant::now();
        let mut state = AudioState::default();

        assert_eq!(
            state.handle(AudioCommand::SetWhiteNoise(WhiteNoise::Ticking), now),
            AudioEffect::LoopWhiteNoise(WhiteNoise::Ticking)
        );
        assert!(state.resume_at.is_none());

        assert_eq!(
            state.handle(AudioCommand::PlayChime(Chime::Complete), now),
            AudioEffect::PlayChime(Chime::Complete)
        );
        assert!(state.resume_at.is_some());

        assert_eq!(
            state.handle(AudioCommand::SetWhiteNoise(WhiteNoise::Ticking), now),
            AudioEffect::None
        );

        assert_eq!(state.on_timeout(), AudioEffect::LoopWhiteNoise(WhiteNoise::Ticking));
        assert!(state.resume_at.is_none());

        state.handle(AudioCommand::PlayChime(Chime::FocusDone), now);
        assert!(state.resume_at.is_some());
        assert_eq!(
            state.handle(AudioCommand::SetWhiteNoise(WhiteNoise::Off), now),
            AudioEffect::Stop
        );
        assert!(state.resume_at.is_none());
        assert_eq!(state.on_timeout(), AudioEffect::None);
    }

    #[test]
    fn linux_audio_cache_dir_scopes_tmp_fallback_by_uid() {
        use std::ffi::OsStr;
        use std::path::{Path, PathBuf};

        let tmp = Path::new("/tmp");
        assert_eq!(
            linux::audio_cache_dir(Some(OsStr::new("/run/user/1000")), tmp, 1000),
            PathBuf::from("/run/user/1000").join("fern-audio")
        );
        assert_eq!(
            linux::audio_cache_dir(None, tmp, 1000),
            PathBuf::from("/tmp").join("fern-audio-1000")
        );
        assert_eq!(
            linux::audio_cache_dir(Some(OsStr::new("")), tmp, 42),
            PathBuf::from("/tmp").join("fern-audio-42")
        );
    }
}
