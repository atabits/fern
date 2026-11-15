fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");
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

    // The app's icon and name in the Windows executable.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("../../assets/icon/icon.ico")
            .set("ProductName", "Fern")
            .set("FileDescription", "Fern — tasks, calendar, habits and focus");
        resource.compile().expect("failed to embed the Windows resources");
    }
}
