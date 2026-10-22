fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // Embed the icon and version info into Windows executables.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Hex Editor")
            .set("FileDescription", "Hex Editor")
            .set("InternalName", "hexeditor");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows resources: {e}");
        }
    }
}
