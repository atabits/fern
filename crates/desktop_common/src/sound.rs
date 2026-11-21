//! Pleasant synthesized completion and Pomodoro chimes (Windows `winmm` `PlaySoundW`).

use fern_core::Store;

const SETTING: &str = "completion_sound";

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
    fn synthesized_wav_has_valid_riff_header() {
        let wav = synthesize_wav(&[(784.0, 0.0, 0.1)], 0.12, 0.2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert!(wav.len() > 44);
    }
}
