use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__fernAudio"], js_name = playOnce, catch)]
    fn js_play_once(wav: &[u8]) -> Result<(), JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__fernAudio"], js_name = playLoop, catch)]
    fn js_play_loop(wav: &[u8]) -> Result<(), JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__fernAudio"], js_name = stopLoop, catch)]
    fn js_stop_loop() -> Result<(), JsValue>;
}

pub(super) fn play_once(wav: &[u8]) {
    let _ = js_play_once(wav);
}

pub(super) fn play_loop(wav: &[u8]) {
    let _ = js_play_loop(wav);
}

pub(super) fn stop_loop() {
    let _ = js_stop_loop();
}
