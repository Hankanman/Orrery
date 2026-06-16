//! gpui_linux links `libxkbcommon-x11` unconditionally (keymap handling), even
//! on a Wayland-only build. Fedora ships the runtime `libxkbcommon-x11.so.0`
//! but the `.so` dev symlink only comes with `libxkbcommon-x11-devel`. Rather
//! than require that package, synthesize the dev symlink in OUT_DIR and point
//! the linker at it. No root needed; the SONAME satisfies the runtime loader too.
use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let link_name = Path::new(&out_dir).join("libxkbcommon-x11.so");

    // If the system already provides the dev symlink, do nothing extra.
    let dev_present = ["/usr/lib64", "/usr/lib", "/lib64", "/lib"]
        .iter()
        .any(|d| Path::new(d).join("libxkbcommon-x11.so").exists());

    if !dev_present {
        let runtime = ["/usr/lib64", "/usr/lib", "/lib64", "/lib"]
            .iter()
            .map(|d| Path::new(d).join("libxkbcommon-x11.so.0"))
            .find(|p| p.exists());

        match runtime {
            Some(target) => {
                let _ = std::fs::remove_file(&link_name);
                if let Err(e) = std::os::unix::fs::symlink(&target, &link_name) {
                    println!("cargo:warning=could not create libxkbcommon-x11.so symlink: {e}");
                }
                println!("cargo:rustc-link-search=native={out_dir}");
            }
            None => {
                println!(
                    "cargo:warning=libxkbcommon-x11.so.0 not found; install libxkbcommon-x11-devel"
                );
            }
        }
    }
}
