//! Embeds the application icon and version information in the Windows
//! executable. Only when building *for* Windows *on* Windows (where the
//! resource compiler is): Linux and macOS builds, and cross-checks of the
//! Windows target from elsewhere, skip it. A missing resource compiler is
//! a warning, not a failure.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/buildrail.ico");
    let for_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !for_windows || !cfg!(windows) {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/buildrail.ico")
        .set("ProductName", "BuildRail")
        .set("FileDescription", "BuildRail")
        .set("LegalCopyright", "MIT licence");
    if let Err(error) = resource.compile() {
        println!("cargo:warning=the exe has no icon or version information: {error}");
    }
}
