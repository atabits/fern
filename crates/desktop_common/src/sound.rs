//! Pleasant synthesized completion and Pomodoro chimes (Windows `winmm` `PlaySoundW`).

use fern_core::Store;

const SETTING: &str = "completion_sound";
const WHITE_NOISE_SETTING: &str = "focus_white_noise";

/// Ambient focus sound presets (pure-Rust procedural PCM, zero external assets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WhiteNoise {
    #[default]
    Off,
    Rain,
    Ocean,
    Ticking,
}

impl WhiteNoise {
    pub const ALL: [Self; 4] = [Self::Off, Self::Rain, Self::Ocean, Self::Ticking];

    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Rain => "rain",
            Self::Ocean => "ocean",
            Self::Ticking => "ticking",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "rain" => Self::Rain,
            "ocean" => Self::Ocean,
            "ticking" => Self::Ticking,
            _ => Self::Off,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Rain,
            Self::Rain => Self::Ocean,
            Self::Ocean => Self::Ticking,
            Self::Ticking => Self::Off,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Rain => 1,
            Self::Ocean => 2,
            Self::Ticking => 3,
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
pub fn sync_white_noise(active: bool, preset: WhiteNoise) {
    #[cfg(all(windows, not(test)))]
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        static PLAYING: AtomicU8 = AtomicU8::new(0);
        let desired = if active { preset.index() as u8 } else { 0 };
        let prev = PLAYING.swap(desired, Ordering::SeqCst);
        if prev == desired {
            return;
        }
        match if active { preset } else { WhiteNoise::Off } {
            WhiteNoise::Off => stop_wav(),
            other => play_wav_loop(white_noise_wav(other)),
        }
    }
    #[cfg(not(all(windows, not(test))))]
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

pub fn set_enabled(store: &Store, on: bool) -> anyhow::Result<()> {
    store.set_setting(SETTING, if on { "1" } else { "0" })
}

/// Plays a short, crisp chime when a task or habit is completed.
pub fn play_complete(store: &Store) {
    if enabled(store) {
        #[cfg(all(windows, not(test)))]
        play_wav(complete_wav());
    }
}

/// Plays a warm three-note chime when a Pomodoro session finishes.
pub fn play_focus_done(store: &Store) {
    if enabled(store) {
        #[cfg(all(windows, not(test)))]
        play_wav(focus_done_wav());
    }
}

/// Generates a 16-bit mono 44.1 kHz PCM WAV buffer for a sequence of `(freq_hz, start_s, dur_s)` notes.
#[cfg_attr(not(windows), allow(dead_code))]
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
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
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

#[cfg(all(windows, not(test)))]
fn complete_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    // Crisp two-note upward tick chime (G5 -> C6).
    WAV.get_or_init(|| synthesize_wav(&[(783.99, 0.0, 0.11), (1046.50, 0.055, 0.15)], 0.21, 0.22))
}

#[cfg(all(windows, not(test)))]
fn focus_done_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    // Warm three-note major triad bell (C5 -> E5 -> G5).
    WAV.get_or_init(|| {
        synthesize_wav(&[(523.25, 0.0, 0.22), (659.25, 0.09, 0.24), (783.99, 0.18, 0.32)], 0.52, 0.24)
    })
}

/// Generates a seamless 4-second 16-bit mono 22.05 kHz PCM WAV loop for an ambient focus preset.
#[cfg_attr(not(windows), allow(dead_code))]
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
    // Deterministic xorshift64 PRNG so noise is reproducible with zero dependencies.
    let mut state: u64 = 0x5EED_CAFE_BABE_1234;
    let mut next_white = || -> f32 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state as f64 / u64::MAX as f64) * 2.0 - 1.0) as f32
    };

    match preset {
        WhiteNoise::Off => {}
        WhiteNoise::Rain => {
            // Soft Paul Kellet pink-noise approximation.
            let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
            for s in &mut samples {
                let w = next_white();
                b0 = 0.99765 * b0 + w * 0.099046;
                b1 = 0.96300 * b1 + w * 0.2965164;
                b2 = 0.57000 * b2 + w * 1.0526913;
                *s = (b0 + b1 + b2 + w * 0.1848) * 0.045;
            }
            // Crossfade loop seam over 0.25s for seamless playback.
            let fade = (SAMPLE_RATE as usize) / 4;
            for i in 0..fade {
                let alpha = i as f32 / fade as f32;
                let tail = samples[num_samples - fade + i];
                samples[i] = samples[i] * alpha + tail * (1.0 - alpha);
            }
        }
        WhiteNoise::Ocean => {
            // Brown noise gently modulated by a 4-second sinusoidal wave envelope.
            let mut acc = 0.0f32;
            for (i, s) in samples.iter_mut().enumerate() {
                acc = (acc + next_white() * 0.04).clamp(-1.0, 1.0) * 0.995;
                let phase = std::f32::consts::TAU * (i as f32 / num_samples as f32);
                let wave = 0.55 + 0.45 * (0.5 - 0.5 * phase.cos());
                *s = acc * wave * 0.22;
            }
        }
        WhiteNoise::Ticking => {
            // Warm wooden mechanical clock tick once per second.
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

#[cfg(all(windows, not(test)))]
fn white_noise_wav(preset: WhiteNoise) -> &'static [u8] {
    use std::sync::OnceLock;
    static RAIN: OnceLock<Vec<u8>> = OnceLock::new();
    static OCEAN: OnceLock<Vec<u8>> = OnceLock::new();
    static TICKING: OnceLock<Vec<u8>> = OnceLock::new();
    match preset {
        WhiteNoise::Rain => RAIN.get_or_init(|| synthesize_ambient(WhiteNoise::Rain)),
        WhiteNoise::Ocean => OCEAN.get_or_init(|| synthesize_ambient(WhiteNoise::Ocean)),
        WhiteNoise::Ticking => TICKING.get_or_init(|| synthesize_ambient(WhiteNoise::Ticking)),
        WhiteNoise::Off => &[],
    }
}

#[cfg(all(windows, not(test)))]
fn play_wav(wav: &'static [u8]) {
    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, fdw_sound: u32) -> i32;
    }

    unsafe {
        PlaySoundW(wav.as_ptr().cast(), core::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

#[cfg(all(windows, not(test)))]
fn play_wav_loop(wav: &'static [u8]) {
    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;
    const SND_LOOP: u32 = 0x0008;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, fdw_sound: u32) -> i32;
    }

    unsafe {
        PlaySoundW(
            wav.as_ptr().cast(),
            core::ptr::null_mut(),
            SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT,
        );
    }
}

#[cfg(all(windows, not(test)))]
fn stop_wav() {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, fdw_sound: u32) -> i32;
    }

    unsafe {
        PlaySoundW(core::ptr::null(), core::ptr::null_mut(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_sound_defaults_on_and_persists() {
        let store = Store::open_in_memory().unwrap();
        assert!(enabled(&store));
        set_enabled(&store, false).unwrap();
        assert!(!enabled(&store));
        set_enabled(&store, true).unwrap();
        assert!(enabled(&store));
    }

    #[test]
    fn white_noise_defaults_off_and_persists() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Off);
        set_white_noise(&store, WhiteNoise::Rain).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Rain);
        set_white_noise(&store, WhiteNoise::Ocean).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Ocean);
        set_white_noise(&store, WhiteNoise::Ticking).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Ticking);
    }

    #[test]
    fn synthesized_wav_has_valid_riff_header() {
        let wav = synthesize_wav(&[(784.0, 0.0, 0.1)], 0.12, 0.2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert!(wav.len() > 44);
        for preset in [WhiteNoise::Rain, WhiteNoise::Ocean, WhiteNoise::Ticking] {
            let amb = synthesize_ambient(preset);
            assert_eq!(&amb[0..4], b"RIFF");
            assert_eq!(&amb[8..12], b"WAVE");
            assert!(amb.len() > 44);
        }
    }
}
