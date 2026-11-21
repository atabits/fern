//! Pleasant synthesized completion chimes, Pomodoro chimes, and focus ticking across Windows, macOS, and Linux.

use fern_core::Store;

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
    #[cfg(not(test))]
    {
        let desired = if active { preset } else { WhiteNoise::Off };
        if desired == WhiteNoise::Off && AUDIO_TX.get().is_none() {
            return;
        }
        send_audio(AudioCommand::SetWhiteNoise(desired));
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

pub fn set_enabled(store: &Store, on: bool) -> anyhow::Result<()> {
    store.set_setting(SETTING, if on { "1" } else { "0" })
}

/// Plays a short, crisp chime when a task or habit is completed.
pub fn play_complete(store: &Store) {
    if enabled(store) {
        #[cfg(not(test))]
        send_audio(AudioCommand::PlayChime(Chime::Complete));
    }
}

/// Plays a warm three-note chime when a Pomodoro session finishes.
pub fn play_focus_done(store: &Store) {
    if enabled(store) {
        #[cfg(not(test))]
        send_audio(AudioCommand::PlayChime(Chime::FocusDone));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chime {
    Complete,
    FocusDone,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioCommand {
    SetWhiteNoise(WhiteNoise),
    PlayChime(Chime),
}

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
#[derive(Default)]
struct AudioState {
    desired_ambient: WhiteNoise,
    resume_at: Option<std::time::Instant>,
}

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

#[cfg(not(test))]
static AUDIO_TX: std::sync::OnceLock<std::sync::mpsc::Sender<AudioCommand>> =
    std::sync::OnceLock::new();

#[cfg(not(test))]
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

#[cfg(not(test))]
fn audio_worker(rx: std::sync::mpsc::Receiver<AudioCommand>) {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Instant;

    let mut state = AudioState::default();
    let mut backend = AudioBackend::new();
    let mut apply = |effect: AudioEffect| match effect {
        AudioEffect::None => {}
        AudioEffect::Stop => backend.stop(),
        AudioEffect::LoopWhiteNoise(preset) => backend.play_loop(white_noise_wav(preset)),
        AudioEffect::PlayChime(Chime::Complete) => backend.play_once("complete", complete_wav()),
        AudioEffect::PlayChime(Chime::FocusDone) => backend.play_once("focus_done", focus_done_wav()),
    };

    loop {
        if let Some(deadline) = state.resume_at {
            let wait = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(cmd) => apply(state.handle(cmd, Instant::now())),
                Err(RecvTimeoutError::Timeout) => apply(state.on_timeout()),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => apply(state.handle(cmd, Instant::now())),
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

#[cfg(not(test))]
fn complete_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    // Crisp two-note upward tick chime (G5 -> C6).
    WAV.get_or_init(|| synthesize_wav(&[(783.99, 0.0, 0.11), (1046.50, 0.055, 0.15)], 0.21, 0.22))
}

#[cfg(not(test))]
fn focus_done_wav() -> &'static [u8] {
    use std::sync::OnceLock;
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    // Warm three-note major triad bell (C5 -> E5 -> G5).
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

#[cfg(not(test))]
fn white_noise_wav(preset: WhiteNoise) -> &'static [u8] {
    use std::sync::OnceLock;
    static TICKING: OnceLock<Vec<u8>> = OnceLock::new();
    match preset {
        WhiteNoise::Ticking => TICKING.get_or_init(|| synthesize_ambient(WhiteNoise::Ticking)),
        WhiteNoise::Off => &[],
    }
}

#[cfg(all(windows, not(test)))]
struct AudioBackend;

#[cfg(all(windows, not(test)))]
impl AudioBackend {
    fn new() -> Self {
        Self
    }

    fn play_once(&mut self, _tag: &str, wav: &'static [u8]) {
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

    fn play_loop(&mut self, wav: &'static [u8]) {
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

    fn stop(&mut self) {
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, fdw_sound: u32) -> i32;
        }

        unsafe {
            PlaySoundW(core::ptr::null(), core::ptr::null_mut(), 0);
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
#[derive(Default)]
struct AudioBackend {
    active: Option<objc2::rc::Retained<objc2_app_kit::NSSound>>,
}

#[cfg(all(target_os = "macos", not(test)))]
impl AudioBackend {
    fn new() -> Self {
        Self::default()
    }

    fn play_data(&mut self, wav: &'static [u8], loops: bool) {
        use objc2::AnyThread as _;
        use objc2_app_kit::NSSound;
        use objc2_foundation::NSData;

        self.stop();
        if wav.is_empty() {
            return;
        }
        let data = NSData::with_bytes(wav);
        if let Some(sound) = NSSound::initWithData(NSSound::alloc(), &data) {
            sound.setLoops(loops);
            if sound.play() {
                self.active = Some(sound);
            }
        }
    }

    fn play_once(&mut self, _tag: &str, wav: &'static [u8]) {
        self.play_data(wav, false);
    }

    fn play_loop(&mut self, wav: &'static [u8]) {
        self.play_data(wav, true);
    }

    fn stop(&mut self) {
        if let Some(sound) = self.active.take() {
            sound.stop();
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
#[derive(Default)]
struct AudioBackend {
    chime_child: Option<std::process::Child>,
    loop_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    loop_child: Option<std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>>,
    loop_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(target_os = "linux", not(test)))]
#[derive(Clone, Copy)]
enum LinuxPlayer {
    PwPlay,
    Pulse,
    Alsa,
}

#[cfg(all(target_os = "linux", not(test)))]
fn has_binary(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

#[cfg(all(target_os = "linux", not(test)))]
fn linux_player() -> Option<LinuxPlayer> {
    static PLAYER: std::sync::OnceLock<Option<LinuxPlayer>> = std::sync::OnceLock::new();
    *PLAYER.get_or_init(|| {
        if has_binary("pw-play") {
            Some(LinuxPlayer::PwPlay)
        } else if has_binary("paplay") {
            Some(LinuxPlayer::Pulse)
        } else if has_binary("aplay") {
            Some(LinuxPlayer::Alsa)
        } else {
            None
        }
    })
}

#[cfg(all(target_os = "linux", not(test)))]
fn cached_wav_path(tag: &str, wav: &[u8]) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("fern-audio");
    std::fs::create_dir_all(&base).ok()?;
    let path = base.join(format!("{tag}.wav"));
    if path.metadata().ok().map(|m| m.len()) != Some(wav.len() as u64) {
        std::fs::write(&path, wav).ok()?;
    }
    Some(path)
}

#[cfg(all(target_os = "linux", not(test)))]
impl AudioBackend {
    fn new() -> Self {
        Self::default()
    }

    fn stop_chime(&mut self) {
        if let Some(mut child) = self.chime_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn stop_loop(&mut self) {
        if let Some(flag) = self.loop_stop.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(slot) = self.loop_child.take()
            && let Ok(mut guard) = slot.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.loop_thread.take() {
            let _ = handle.join();
        }
    }

    fn play_once(&mut self, tag: &str, wav: &'static [u8]) {
        use std::process::{Command, Stdio};

        self.stop();
        let Some(player) = linux_player() else { return };
        let Some(path) = cached_wav_path(tag, wav) else { return };
        let mut cmd = match player {
            LinuxPlayer::PwPlay => {
                let mut c = Command::new("pw-play");
                c.arg(&path);
                c
            }
            LinuxPlayer::Pulse => {
                let mut c = Command::new("paplay");
                c.arg(&path);
                c
            }
            LinuxPlayer::Alsa => {
                let mut c = Command::new("aplay");
                c.args(["-q"]).arg(&path);
                c
            }
        };
        if let Ok(child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            self.chime_child = Some(child);
        }
    }

    fn play_loop(&mut self, wav: &'static [u8]) {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        self.stop();
        if wav.len() <= 44 {
            return;
        }
        let Some(player) = linux_player() else { return };
        let pcm = &wav[44..];
        let mut cmd = match player {
            LinuxPlayer::PwPlay => {
                let mut c = Command::new("pw-play");
                c.args(["--rate=22050", "--channels=1", "--format=s16", "-"]);
                c
            }
            LinuxPlayer::Pulse if has_binary("pacat") => {
                let mut c = Command::new("pacat");
                c.args(["--playback", "--rate=22050", "--channels=1", "--format=s16le", "--raw"]);
                c
            }
            LinuxPlayer::Pulse => {
                // Fallback when only `paplay` is present without `pacat`.
                let mut c = Command::new("paplay");
                c.args(["--rate=22050", "--channels=1", "--format=s16le", "--raw"]);
                c
            }
            LinuxPlayer::Alsa => {
                let mut c = Command::new("aplay");
                c.args(["-q", "-f", "S16_LE", "-r", "22050", "-c", "1", "-"]);
                c
            }
        };
        let Ok(mut child) = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let stop_bg = Arc::clone(&stop);
        let child_bg = Arc::clone(&child_slot);
        let handle = std::thread::Builder::new()
            .name("fern-audio-loop".into())
            .spawn(move || {
                'outer: while !stop_bg.load(Ordering::Relaxed) {
                    for chunk in pcm.chunks(4096) {
                        if stop_bg.load(Ordering::Relaxed) || stdin.write_all(chunk).is_err() {
                            break 'outer;
                        }
                    }
                }
                drop(stdin);
                if let Ok(mut guard) = child_bg.lock()
                    && let Some(mut c) = guard.take()
                {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            })
            .ok();
        self.loop_stop = Some(stop);
        self.loop_child = Some(child_slot);
        self.loop_thread = handle;
    }

    fn stop(&mut self) {
        self.stop_chime();
        self.stop_loop();
    }
}

#[cfg(all(not(any(windows, target_os = "macos", target_os = "linux")), not(test)))]
struct AudioBackend;

#[cfg(all(not(any(windows, target_os = "macos", target_os = "linux")), not(test)))]
impl AudioBackend {
    fn new() -> Self {
        Self
    }
    fn play_once(&mut self, _tag: &str, _wav: &'static [u8]) {}
    fn play_loop(&mut self, _wav: &'static [u8]) {}
    fn stop(&mut self) {}
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
        set_white_noise(&store, WhiteNoise::Ticking).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Ticking);
        set_white_noise(&store, WhiteNoise::Off).unwrap();
        assert_eq!(white_noise(&store), WhiteNoise::Off);

        // Removed/unknown presets gracefully fall back to Off.
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

        // Chime interrupts Win32 PlaySoundW loop and schedules an automatic resume.
        assert_eq!(
            state.handle(AudioCommand::PlayChime(Chime::Complete), now),
            AudioEffect::PlayChime(Chime::Complete)
        );
        assert!(state.resume_at.is_some());

        // Periodic sync ticks for the same preset do not cut off the chime.
        assert_eq!(
            state.handle(AudioCommand::SetWhiteNoise(WhiteNoise::Ticking), now),
            AudioEffect::None
        );

        // Once the chime finishes, white noise resumes automatically.
        assert_eq!(state.on_timeout(), AudioEffect::LoopWhiteNoise(WhiteNoise::Ticking));
        assert!(state.resume_at.is_none());

        // Stopping white noise during a chime cancels pending resume.
        state.handle(AudioCommand::PlayChime(Chime::FocusDone), now);
        assert!(state.resume_at.is_some());
        assert_eq!(
            state.handle(AudioCommand::SetWhiteNoise(WhiteNoise::Off), now),
            AudioEffect::Stop
        );
        assert!(state.resume_at.is_none());
        assert_eq!(state.on_timeout(), AudioEffect::None);
    }
}
