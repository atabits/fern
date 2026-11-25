fn main() {
    println!("cargo:rerun-if-changed=../../.github/changes");

    let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
    let path = format!("../../.github/changes/{version}.md");
    let notes = match std::fs::read_to_string(&path) {
        Ok(notes) => notes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => panic!("failed to read {path}: {err}"),
    };
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("build output directory"));
    std::fs::write(out.join("release-notes.md"), notes).expect("failed to bundle release notes");
}
