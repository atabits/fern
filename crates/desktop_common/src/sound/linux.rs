use std::ffi::OsStr;
use std::path::{Path, PathBuf};
#[cfg(all(target_os = "linux", not(test)))]
use std::process::{Child, Command, Stdio};
#[cfg(all(target_os = "linux", not(test)))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(all(target_os = "linux", not(test)))]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(all(target_os = "linux", not(test)))]
use std::time::{Duration, Instant};

pub(super) fn audio_cache_dir(
    xdg_runtime_dir: Option<&OsStr>,
    temp_dir: &Path,
    uid: u32,
) -> PathBuf {
    if let Some(dir) = xdg_runtime_dir.filter(|d| !d.is_empty()) {
        PathBuf::from(dir).join("fern-audio")
    } else {
        temp_dir.join(format!("fern-audio-{uid}"))
    }
}

#[cfg(all(target_os = "linux", not(test)))]
#[derive(Default)]
pub(super) struct AudioBackend {
    chime_child: Option<Child>,
    chime_reap_at: Option<Instant>,
    loop_stop: Option<Arc<AtomicBool>>,
    loop_child: Option<Arc<Mutex<Option<Child>>>>,
    loop_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(target_os = "linux", not(test)))]
#[derive(Clone, Copy)]
enum LinuxPlayer {
    PwPlay,
    Pulse { pacat: bool },
    Alsa,
}

#[cfg(all(target_os = "linux", not(test)))]
fn has_binary(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

#[cfg(all(target_os = "linux", not(test)))]
fn linux_players() -> &'static [LinuxPlayer] {
    static PLAYERS: OnceLock<Vec<LinuxPlayer>> = OnceLock::new();
    PLAYERS
        .get_or_init(|| {
            let mut list = Vec::with_capacity(3);
            if has_binary("pw-play") {
                list.push(LinuxPlayer::PwPlay);
            }
            if has_binary("paplay") {
                list.push(LinuxPlayer::Pulse { pacat: has_binary("pacat") });
            }
            if has_binary("aplay") {
                list.push(LinuxPlayer::Alsa);
            }
            list
        })
        .as_slice()
}

#[cfg(all(target_os = "linux", not(test)))]
fn linux_chime_command(player: LinuxPlayer, path: &Path) -> Command {
    match player {
        LinuxPlayer::PwPlay => {
            let mut c = Command::new("pw-play");
            c.env(
                "PIPEWIRE_PROPS",
                "{ application.name = \"Fern\" media.role = \"Notification\" }",
            )
            .arg(path);
            c
        }
        LinuxPlayer::Pulse { .. } => {
            let mut c = Command::new("paplay");
            c.args(["--client-name=Fern", "--stream-name=Notification"]).arg(path);
            c
        }
        LinuxPlayer::Alsa => {
            let mut c = Command::new("aplay");
            c.args(["-q"]).arg(path);
            c
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
fn linux_loop_command(player: LinuxPlayer) -> Command {
    const PULSE_RAW_ARGS: [&str; 6] = [
        "--client-name=Fern",
        "--stream-name=Focus Noise",
        "--rate=22050",
        "--channels=1",
        "--format=s16le",
        "--raw",
    ];
    match player {
        LinuxPlayer::PwPlay => {
            let mut c = Command::new("pw-play");
            c.env("PIPEWIRE_PROPS", "{ application.name = \"Fern\" media.role = \"Music\" }")
                .args(["--rate=22050", "--channels=1", "--format=s16", "-"]);
            c
        }
        LinuxPlayer::Pulse { pacat } => {
            let mut c = Command::new(if pacat { "pacat" } else { "paplay" });
            if pacat {
                c.arg("--playback");
            }
            c.args(PULSE_RAW_ARGS);
            c
        }
        LinuxPlayer::Alsa => {
            let mut c = Command::new("aplay");
            c.args(["-q", "-f", "S16_LE", "-r", "22050", "-c", "1", "-"]);
            c
        }
    }
}

#[cfg(all(target_os = "linux", not(test)))]
fn cached_wav_path(tag: &str, wav: &[u8]) -> Option<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};

    let uid = std::fs::metadata("/proc/self")
        .ok()
        .map(|m| m.uid())
        .unwrap_or_else(std::process::id);
    let xdg = std::env::var_os("XDG_RUNTIME_DIR");
    let base = audio_cache_dir(xdg.as_deref(), &std::env::temp_dir(), uid);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&base)
        .ok()?;
    let path = base.join(format!("{tag}.wav"));
    if path.metadata().ok().map(|m| m.len()) != Some(wav.len() as u64) {
        std::fs::write(&path, wav).ok()?;
    }
    Some(path)
}

#[cfg(all(target_os = "linux", not(test)))]
impl AudioBackend {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn stop_chime(&mut self) {
        self.chime_reap_at = None;
        if let Some(mut child) = self.chime_child.take()
            && !matches!(child.try_wait(), Ok(Some(_)))
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub(super) fn reap_deadline(&self) -> Option<Instant> {
        self.chime_reap_at
    }

    pub(super) fn poll_reap(&mut self, now: Instant) {
        let Some(child) = self.chime_child.as_mut() else {
            self.chime_reap_at = None;
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => {
                self.chime_child = None;
                self.chime_reap_at = None;
            }
            Ok(None) => {
                if self.chime_reap_at.is_none_or(|t| now >= t) {
                    self.chime_reap_at = Some(now + Duration::from_millis(250));
                }
            }
        }
    }

    fn stop_loop(&mut self) {
        if let Some(flag) = self.loop_stop.take() {
            flag.store(true, Ordering::Relaxed);
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

    pub(super) fn play_once(&mut self, tag: &str, wav: &'static [u8]) {
        self.stop();
        let players = linux_players();
        if players.is_empty() {
            return;
        }
        let Some(path) = cached_wav_path(tag, wav) else { return };
        for &player in players {
            if let Ok(child) = linux_chime_command(player, &path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                self.chime_child = Some(child);
                self.chime_reap_at = Some(Instant::now() + Duration::from_millis(250));
                break;
            }
        }
    }

    pub(super) fn play_loop(&mut self, wav: &'static [u8]) {
        use std::io::Write as _;

        self.stop();
        if wav.len() <= 44 {
            return;
        }
        let pcm = &wav[44..];
        let Some(mut child) = linux_players().iter().find_map(|&player| {
            linux_loop_command(player)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        }) else {
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

    pub(super) fn stop(&mut self) {
        self.stop_chime();
        self.stop_loop();
    }
}
