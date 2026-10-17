//! Собирает таблицу иконок из `assets/icons`, чтобы новый SVG не нужно было регистрировать вручную.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("каталог assets/icons")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".svg"))
        .collect();
    names.sort();

    let mut out = String::from("&[\n");
    for name in names {
        let path = dir.join(&name);
        writeln!(out, "    (\"icons/{name}\", include_bytes!({:?}) as &[u8]),", path.display().to_string()).unwrap();
    }
    out.push_str("]\n");

    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("icons.rs");
    std::fs::write(dest, out).unwrap();
}
