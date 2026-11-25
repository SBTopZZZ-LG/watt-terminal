use std::ffi::OsStr;
use std::path::{Path, PathBuf};

// Microsoft's ConPTY host, copied next to the exe and into `deps/` (test exes run from there).
// portable-pty loads `conpty.dll` by bare name and silently falls back to inbox conhost if missing.
const CONPTY_FILES: [&str; 2] = ["conpty.dll", "OpenConsole.exe"];

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        stage_conpty();
    }
    tauri_build::build()
}

fn stage_conpty() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = profile_dir_from_out_dir(&out_dir)
        .unwrap_or_else(|| panic!("cannot find cargo profile dir from OUT_DIR {}", out_dir.display()));
    let vendor_dir = manifest_dir.join("vendor").join("conpty").join("x64");

    for name in CONPTY_FILES {
        let src = vendor_dir.join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        if !src.is_file() {
            panic!(
                "missing vendored {} (expected at {}); see src-tauri/vendor/conpty/README.md",
                name,
                src.display()
            );
        }
    }
    for name in CONPTY_FILES {
        let src = vendor_dir.join(name);
        for dir in [profile_dir.clone(), profile_dir.join("deps")] {
            std::fs::create_dir_all(&dir).expect("create profile dir");
            let dst = dir.join(name);
            std::fs::copy(&src, &dst)
                .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", src.display(), dst.display()));
        }
    }
}

// Same derivation as tauri-build: OUT_DIR is `<target>/<profile>/build/<pkg>-<hash>/out`, so the
// profile dir is the parent of the `build` dir (works with --target too).
fn profile_dir_from_out_dir(out_dir: &Path) -> Option<PathBuf> {
    out_dir
        .ancestors()
        .find(|p| p.file_name() == Some(OsStr::new("build")))
        .and_then(|build| build.parent())
        .map(Path::to_path_buf)
}
