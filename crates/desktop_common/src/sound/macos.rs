use std::cell::RefCell;

thread_local! {
    static MACOS_ACTIVE_SOUND: RefCell<Option<objc2::rc::Retained<objc2_app_kit::NSSound>>> =
        const { RefCell::new(None) };
}

pub(super) struct AudioBackend;

impl AudioBackend {
    pub(super) fn new() -> Self {
        Self
    }

    fn play_data(&mut self, wav: &'static [u8], loops: bool) {
        dispatch2::DispatchQueue::main().exec_async(move || {
            objc2::rc::autoreleasepool(|_| {
                use objc2::AnyThread as _;
                use objc2_app_kit::NSSound;
                use objc2_foundation::NSData;

                MACOS_ACTIVE_SOUND.with(|slot| {
                    let prev = slot.borrow_mut().take();
                    if let Some(prev) = prev {
                        prev.stop();
                    }
                    if wav.is_empty() {
                        return;
                    }
                    let data = NSData::with_bytes(wav);
                    if let Some(sound) = NSSound::initWithData(NSSound::alloc(), &data) {
                        sound.setLoops(loops);
                        if sound.play() {
                            *slot.borrow_mut() = Some(sound);
                        }
                    }
                });
            });
        });
    }

    pub(super) fn play_once(&mut self, _tag: &str, wav: &'static [u8]) {
        self.play_data(wav, false);
    }

    pub(super) fn play_loop(&mut self, wav: &'static [u8]) {
        self.play_data(wav, true);
    }

    pub(super) fn stop(&mut self) {
        dispatch2::DispatchQueue::main().exec_async(|| {
            objc2::rc::autoreleasepool(|_| {
                MACOS_ACTIVE_SOUND.with(|slot| {
                    let prev = slot.borrow_mut().take();
                    if let Some(prev) = prev {
                        prev.stop();
                    }
                });
            });
        });
    }

    pub(super) fn reap_deadline(&self) -> Option<std::time::Instant> {
        None
    }

    pub(super) fn poll_reap(&mut self, _now: std::time::Instant) {}
}
