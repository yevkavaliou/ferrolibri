//! Embeds the application icon and version information into the Windows
//! executable. Other targets need nothing.

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/app-icon.ico")
        .set("ProductName", "Rulibre")
        .set(
            "FileDescription",
            "Rulibre: browse a Calibre library and send books to a reader",
        )
        .set("OriginalFilename", "rulibre-gui.exe")
        .set("LegalCopyright", "MIT License");
    // Version fields come from the package version (CARGO_PKG_VERSION).
    if let Err(error) = resource.compile() {
        panic!("could not embed Windows resources: {error}");
    }
}
