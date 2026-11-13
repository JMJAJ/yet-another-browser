fn main() {
    slint_build::compile("ui/app.slint").expect("slint compile");
    #[cfg(windows)]
    stage_cef();
}

/// With `--features engine-cef`, copy CEF's runtime files (libcef.dll,
/// resources, locales) next to the built exe so `cargo run` works. Files
/// already in place are skipped.
#[cfg(windows)]
fn stage_cef() {
    use std::path::{Path, PathBuf};
    if std::env::var_os("CARGO_FEATURE_ENGINE_CEF").is_none() {
        return;
    }
    // load libcef.dll on first use, so a missing engine falls back to
    // WebView2 instead of stopping the exe from starting
    println!("cargo::rustc-link-arg-bins=/DELAYLOAD:libcef.dll");
    println!("cargo::rustc-link-lib=delayimp");
    let Some(cef) = std::env::var_os("CEF_PATH").map(PathBuf::from) else { return };
    // OUT_DIR = target/<profile>/build/<pkg>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile) = out.ancestors().nth(3) else { return };
    fn copy(from: &Path, to: &Path) {
        if from.is_dir() {
            let _ = std::fs::create_dir_all(to);
            if let Ok(rd) = std::fs::read_dir(from) {
                for e in rd.flatten() {
                    copy(&e.path(), &to.join(e.file_name()));
                }
            }
        } else {
            let same = to.metadata().ok().map(|m| m.len()) == from.metadata().ok().map(|m| m.len());
            if !same {
                let _ = std::fs::copy(from, to);
            }
        }
    }
    let runtime = ["libcef.dll", "chrome_elf.dll", "d3dcompiler_47.dll", "dxcompiler.dll", "vk_swiftshader.dll",
        "vk_swiftshader_icd.json", "vulkan-1.dll", "icudtl.dat", "v8_context_snapshot.bin", "resources.pak",
        "chrome_100_percent.pak", "chrome_200_percent.pak", "locales"];
    for f in runtime {
        copy(&cef.join(f), &profile.join(f));
    }
    println!("cargo::rerun-if-env-changed=CEF_PATH");
}
