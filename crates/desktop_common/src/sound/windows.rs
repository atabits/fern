#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(psz_sound: *const u16, hmod: *mut core::ffi::c_void, fdw_sound: u32) -> i32;
}

const SND_ASYNC: u32 = 0x0001;
const SND_NODEFAULT: u32 = 0x0002;
const SND_MEMORY: u32 = 0x0004;
const SND_LOOP: u32 = 0x0008;

fn play_sound_raw(wav: Option<&'static [u8]>, flags: u32) {
    let ptr = wav.map_or(core::ptr::null(), |bytes| bytes.as_ptr().cast());
    unsafe {
        PlaySoundW(ptr, core::ptr::null_mut(), flags);
    }
}

pub(super) struct AudioBackend;

impl AudioBackend {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn play_once(&mut self, _tag: &str, wav: &'static [u8]) {
        play_sound_raw(Some(wav), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }

    pub(super) fn play_loop(&mut self, wav: &'static [u8]) {
        play_sound_raw(Some(wav), SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT);
    }

    pub(super) fn stop(&mut self) {
        play_sound_raw(None, 0);
    }

    pub(super) fn reap_deadline(&self) -> Option<std::time::Instant> {
        None
    }

    pub(super) fn poll_reap(&mut self, _now: std::time::Instant) {}
}
