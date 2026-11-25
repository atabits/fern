fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/icon.ico");

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
