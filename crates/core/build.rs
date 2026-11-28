//! Keys of the code that draws what is kept on disk between runs, given to the code as
//! environment variables: a build that draws differently makes them again rather than reading
//! another's.
//!
//! - PROCRAFT_DRAWING_KEY: the big pictures of our shapes and grains (see `src/big_pictures.rs`),
//!   FNV-1a of the code that draws them and the tables it draws from.
//! - PROCRAFT_PREVIEW_KEY: the brushes' sample strokes (see `src/drawn_previews.rs`), FNV-1a of
//!   all the code a stroke is drawn by (the engine's, the GPU's with its shaders, the core's but
//!   its tests) and of the libraries it is built with.

use std::path::{Path, PathBuf};

/// The files whose code or tables the pictures are drawn by.
const DRAWING: [&str; 3] = ["../engine/src/textures.rs", "../engine/src/stock_tones.rs", "src/stand_ins.rs"];

/// The folders whose code a sample stroke is drawn by, and the file naming the libraries built
/// with.
const PREVIEW_FOLDERS: [&str; 3] = ["../engine/src", "../gpu/src", "src"];
const LOCK: &str = "../../Cargo.lock";

fn main() {
    let drawing: Vec<PathBuf> = DRAWING.iter().map(PathBuf::from).collect();
    println!("cargo:rustc-env=PROCRAFT_DRAWING_KEY={:016x}", key_of(&drawing));
    let mut previews = Vec::new();
    for folder in PREVIEW_FOLDERS {
        println!("cargo:rerun-if-changed={folder}");
        code_in(Path::new(folder), &mut previews);
    }
    println!("cargo:rerun-if-changed={LOCK}");
    previews.sort();
    previews.push(PathBuf::from(LOCK));
    println!("cargo:rustc-env=PROCRAFT_PREVIEW_KEY={:016x}", key_of(&previews));
}

/// FNV-1a of the files at `paths`, their names with them (the same for a checkout with Windows
/// line ends).
fn key_of(paths: &[PathBuf]) -> u64 {
    let mut key = 0xcbf2_9ce4_8422_2325u64;
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read(path).unwrap_or_default();
        let name = path.to_string_lossy().replace('\\', "/");
        for byte in name.bytes().chain(text.into_iter().filter(|&b| b != b'\r')) {
            key = (key ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    key
}

/// The code under `folder` (Rust and shaders), but tests: into `found`.
fn code_in(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                code_in(&path, found);
            }
        } else if (name.ends_with(".rs") || name.ends_with(".wgsl"))
            && name != "tests.rs"
            && !name.ends_with("_tests.rs")
        {
            found.push(path);
        }
    }
}
