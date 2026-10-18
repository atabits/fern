fn main() {
    // The program's name in the Windows executable, which signed binaries must carry.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set("ProductName", "Fern Server").set("FileDescription", "Fern's self-hosted sync server");
        resource.compile().expect("failed to embed the Windows resources");
    }
}
