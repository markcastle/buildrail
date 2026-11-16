//! Defines `cfg(x11_platform)`: Unix desktops that use X11 or Wayland
//! (Linux and the BSDs), i.e. not macOS, iOS or Android.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(x11_platform)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let family = std::env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let unix = family.split(',').any(|f| f == "unix");
    if unix && !matches!(os.as_str(), "macos" | "ios" | "android") {
        println!("cargo::rustc-cfg=x11_platform");
    }
}
