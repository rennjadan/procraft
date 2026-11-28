#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
//! Thousands of brushes brought in from one set file (#415: a phone lagged and closed after a file
//! of thousands): each with a shape, a grain and a preview of its own. Read a brush at a time, each
//! puts its pictures in their files as it comes in, so the heap holds their settings, not every
//! brush's pictures; once in, the library's brushes read their pictures from their files when one
//! draws with them. After a restart the library is read ahead (as the app does on a thread of its
//! own while the gallery shows) and opens at once, its first screen of strokes too. (A test of its
//! own, as it counts every allocation the program makes; the restart is the test run again as a
//! process of its own.)
//!
//! Printed: the heap at its fullest while the file is read and the brushes join the library, what
//! it holds after; after a restart, the time to read it ahead and to open it, the heap at its
//! fullest and held, and the first screen's strokes' time. Measured on a busy PC: 85 MB at the
//! fullest and 69 MB held after (before, 521 MB and 490 MB: every brush's pictures); after a
//! restart read ahead in 4.6 to 13 s, opened in 2 to 3 s, 37 MB at the fullest and 30 MB held
//! (before, 5.9 s to open, 427 MB and 420 MB). Bounds: the heap well under the before; the times
//! with room for a busy machine.

use std::time::Instant;

use procraft_core::{CanvasSession, read_brushes_file};
use procraft_engine::brush::Brush;
use procraft_engine::brushfile::{Pictures, write_set};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const BRUSHES: usize = 3000;
/// The side of each brush's shape and grain in the file, and its preview's size.
const SIDE: usize = 64;
const PREVIEW: (u32, u32) = (212, 65);
const MB: usize = 1 << 20;

/// A grey picture `side` square of its own for `seed`: soft rings, different for each seed.
fn picture_of(seed: usize, side: usize) -> Vec<u8> {
    let rings = 2.0 + (seed % 13) as f32 + (seed / 13 % 7) as f32 * 0.37;
    let half = side as f32 / 2.0;
    (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f32 - half, (i / side) as f32 - half);
            let r = (x * x + y * y).sqrt() / half;
            let soft = (1.0 - r).clamp(0.0, 1.0) * (0.6 + 0.4 * (r * rings * std::f32::consts::TAU).cos());
            let mark = if i % side == seed % side || i / side == seed / side % side { 0.2 } else { 0.0 };
            ((soft + mark).clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect()
}

/// A gray PNG of `pixels`, `w` × `h`.
fn png(w: u32, h: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(pixels).unwrap();
    writer.finish().unwrap();
    out
}

/// The set file: `BRUSHES` brushes, each a round pen with its own shape, grain and preview.
fn set_file() -> Vec<u8> {
    let pictures: Vec<[Vec<u8>; 3]> = (0..BRUSHES)
        .map(|i| {
            let (w, h) = PREVIEW;
            let preview: Vec<u8> = (0..w * h).map(|p| ((p % w + i as u32) % 251) as u8).collect();
            [
                png(SIDE as u32, SIDE as u32, &picture_of(2 * i, SIDE)),
                png(SIDE as u32, SIDE as u32, &picture_of(2 * i + 1, SIDE)),
                png(w, h, &preview),
            ]
        })
        .collect();
    let brushes: Vec<Brush> = (0..BRUSHES)
        .map(|i| {
            let mut b = Brush { name: format!("Many {i}"), ..Brush::round_pen() };
            b.grain.depth = 0.8;
            b
        })
        .collect();
    let list: Vec<(&Brush, &str, Pictures<'_>)> = brushes
        .iter()
        .zip(&pictures)
        .map(|(b, [shape, grain, preview])| {
            let pictures = Pictures {
                shape_png: shape,
                grain_png: Some(grain),
                metallic_png: None,
                roughness_png: None,
                thumbnail_png: preview,
                author_png: None,
                signature_png: None,
                secondary: None,
            };
            (b, "", pictures)
        })
        .collect();
    write_set("Thousands", &list, |i| format!("brush-{i}"))
}

/// The app started again on the brushes' folder `folder` (run as a process of its own, with
/// nothing of the first run held): the library read ahead from its files, as the app does on a
/// thread of its own while the gallery shows; then an artwork opened, the library opened and its
/// rows listed; then the first screen of the big set's rows' strokes. Printed for the first run
/// to read: the time to read it ahead and to open it, the heap at its fullest and held after, the
/// screen's time.
fn opened_again(folder: &str) {
    let profiler = dhat::Profiler::builder().testing().build();
    let t = Instant::now();
    procraft_core::prepare_brushes(folder.to_owned());
    let ahead_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let s = CanvasSession::new(64, 64);
    assert!(s.open_brushes(folder.to_owned()));
    let library = s.brush_library();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;
    let set = library.iter().position(|set| set.brushes.len() == BRUSHES).expect("the set") as u32;
    let opened = dhat::HeapStats::get();
    let t = Instant::now();
    for i in 0..12 {
        assert!(s.brush_preview(set, i, 530, 162).is_some());
    }
    let screen_ms = t.elapsed().as_secs_f64() * 1000.0;
    drop(profiler);
    println!("OPENED {ahead_ms:.0} {open_ms:.0} {} {} {screen_ms:.0}", opened.max_bytes, opened.curr_bytes);
}

#[test]
fn thousands_of_brushes_from_one_file_come_in_and_open_in_bounded_memory_and_time() {
    if let Ok(folder) = std::env::var("PROCRAFT_THOUSANDS_AGAIN") {
        opened_again(&folder);
        return;
    }
    let dir = std::env::temp_dir().join(format!("procraft-thousands-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Thousands.brushset");
    std::fs::write(&path, set_file()).unwrap();
    let file_bytes = std::fs::metadata(&path).unwrap().len() as usize;
    let brushes_dir = dir.join("brushes");
    let folder = brushes_dir.to_string_lossy().into_owned();
    // what the brushes' pictures take drawn (a shape and a grain each, as brushes hold them)
    let side = procraft_engine::textures::TEXTURE_SIZE;
    let pictures_bytes = BRUSHES * 2 * side * side;

    let profiler = dhat::Profiler::builder().testing().build();
    let s = CanvasSession::new(64, 64);
    s.open_brushes(folder.clone());
    let before = dhat::HeapStats::get().curr_bytes;
    let t = Instant::now();
    let read =
        read_brushes_file(path.to_string_lossy().into_owned(), Some("Thousands.brushset".into()), None).expect("read");
    let read_ms = t.elapsed().as_secs_f64() * 1000.0;
    let at_read = dhat::HeapStats::get();
    let added = s.add_brushes_read(read, 0).expect("added");
    assert!(s.keep_brushes(folder.clone()));
    let in_ms = t.elapsed().as_secs_f64() * 1000.0;
    let came_in = dhat::HeapStats::get();
    let held_in = came_in.curr_bytes.saturating_sub(before);
    let library = s.brush_library();
    assert_eq!(library[added as usize].brushes.len(), BRUSHES);
    drop(library);
    drop(s);
    drop(profiler);

    // the app started again
    let again = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "thousands_of_brushes_from_one_file_come_in_and_open_in_bounded_memory_and_time",
            "--nocapture",
        ])
        .env("PROCRAFT_THOUSANDS_AGAIN", &folder)
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&again.stdout);
    let numbers: Vec<f64> = said
        .lines()
        .find_map(|l| l.strip_prefix("OPENED "))
        .unwrap_or_else(|| panic!("opened again: {said}{}", String::from_utf8_lossy(&again.stderr)))
        .split(' ')
        .map(|n| n.parse().unwrap())
        .collect();
    let [ahead_ms, open_ms, open_peak, open_held, screen_ms] = numbers[..] else { panic!("{numbers:?}") };
    let (open_peak, open_held) = (open_peak as usize, open_held as usize);
    eprintln!(
        "{BRUSHES} brushes from a file of {} MB (their pictures drawn {} MB): read in {read_ms:.0} ms, in the          library and kept in {in_ms:.0} ms; heap at its fullest {} MB (after reading {} MB), held after {} MB;          after a restart read ahead in {ahead_ms:.0} ms and opened in {open_ms:.0} ms, heap at its fullest {} MB,          held {} MB; first screen of strokes {screen_ms:.0} ms",
        file_bytes / MB,
        pictures_bytes / MB,
        came_in.max_bytes / MB,
        at_read.curr_bytes.saturating_sub(before) / MB,
        held_in / MB,
        open_peak / MB,
        open_held / MB,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(came_in.max_bytes < pictures_bytes * 2 / 5, "{} MB at its fullest", came_in.max_bytes / MB);
    assert!(held_in < pictures_bytes / 3, "{} MB held", held_in / MB);
    assert!(
        open_peak < pictures_bytes / 4 && open_held < pictures_bytes / 6,
        "{} / {} MB",
        open_peak / MB,
        open_held / MB
    );
    // (times swing threefold on a machine other builds keep busy: bounds with room for that)
    assert!(open_ms < 6000.0 && screen_ms < 1500.0, "{open_ms:.0} / {screen_ms:.0} ms");
    assert!(ahead_ms < 30_000.0, "read ahead in {ahead_ms:.0} ms");
}
