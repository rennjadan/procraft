//! The brushes' sample strokes drawn for hundreds of brushes brought in at once, on a GPU that
//! offers less than most: in bounded memory and time, the device never lost (discord).

use procraft_engine::brush::Brush;
use procraft_gpu::{Gpu, wgpu};

use crate::CanvasSession;

/// A grey picture `side` square of its own for `seed`: soft rings and a speckle, different for each
/// seed (each a picture of its own to the GPU), still compressing well.
fn picture_of(seed: usize, side: usize) -> Vec<u8> {
    let mut x = 0x9e37_79b9_7f4a_7c15u64 ^ (seed as u64).wrapping_mul(0x2545_f491_4f6c_dd1d);
    let rings = 3.0 + (seed % 11) as f32;
    let half = side as f32 / 2.0;
    (0..side * side)
        .map(|i| {
            let (px, py) = ((i % side) as f32 - half, (i / side) as f32 - half);
            let r = (px * px + py * py).sqrt() / half;
            let soft = (1.0 - r).clamp(0.0, 1.0) * (0.6 + 0.4 * (r * rings * std::f32::consts::TAU).cos());
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let speck = if (i / side) % 64 == seed % 64 { (x >> 56) as f32 / 255.0 } else { 0.0 };
            ((soft + speck * 0.3).clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect()
}

/// A set file of `count` brushes, each one of the stock brushes in turn (dry, wet, dual and laid in
/// place among them) with a shape and a grain of its own, `side` square, and its second's own
/// where it has one; none with a preview of its own, so the library draws each.
fn set_of_own_brushes(s: &CanvasSession, count: usize, side: usize) -> Vec<u8> {
    use procraft_engine::brushfile::{Pictures, write_set};
    let stock: Vec<Brush> = {
        let st = s.shared.lock();
        st.library.iter().flat_map(|set| set.brushes.iter().cloned()).collect()
    };
    let png = |seed: usize| crate::brush_files::sized_gray_png(side as u32, side as u32, &picture_of(seed, side));
    let pngs: Vec<[Vec<u8>; 4]> =
        (0..count).map(|i| [png(4 * i), png(4 * i + 1), png(4 * i + 2), png(4 * i + 3)]).collect();
    let brushes: Vec<Brush> = (0..count)
        .map(|i| {
            let b = &stock[(i * 7) % stock.len()];
            Brush { name: format!("Own {i}"), ..b.clone() }
        })
        .collect();
    let list: Vec<(&Brush, &str, Pictures<'_>)> = brushes
        .iter()
        .zip(&pngs)
        .map(|(b, [shape, grain, second_shape, second_grain])| {
            let secondary = b.secondary.as_ref().map(|_| {
                Box::new(Pictures {
                    shape_png: second_shape,
                    grain_png: Some(second_grain),
                    metallic_png: None,
                    roughness_png: None,
                    thumbnail_png: &[],
                    author_png: None,
                    signature_png: None,
                    secondary: None,
                })
            });
            let pictures = Pictures {
                shape_png: shape,
                grain_png: Some(grain),
                metallic_png: None,
                roughness_png: None,
                thumbnail_png: &[],
                author_png: None,
                signature_png: None,
                secondary,
            };
            (b, "", pictures)
        })
        .collect();
    write_set("Own", &list, |i| format!("own-{i}"))
}

/// The GPU's memory its allocator holds now, bytes.
fn reserved(gpu: &Gpu) -> u64 {
    gpu.device.generate_allocator_report().map_or(0, |r| r.total_reserved_bytes)
}

/// What drawing the sample strokes of many brushes of their own came to (see
/// [`previews_of_many_brushes`]).
struct PreviewRun {
    drawn: usize,
    /// The GPU's memory over what it held before, at its highest after the first tenth of the
    /// strokes and after all, and after the library closed, MB.
    first_peak: u64,
    peak: u64,
    after: u64,
    /// The longest a stroke took to draw, and the longest one piece of a stroke's work took on the
    /// GPU (where the strokes' canvas is kept between them), ms.
    longest_stroke: u128,
    longest_piece: u128,
    /// The most the strokes' canvas, kept between them, held of brushes' own images at their size
    /// after a stroke, KB.
    full_kept: u64,
    total: std::time::Duration,
}

/// `count` brushes of their own (`side` square shapes and grains) brought in as a set and every
/// one's sample stroke drawn, as the library's rows ask for them, on `gpu` with `free` bytes of
/// memory free (none: as much as it likes), a 2000 × 1200 canvas open.
fn previews_of_many_brushes(gpu: &Gpu, count: usize, side: usize, free: Option<u64>) -> PreviewRun {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let dir = std::env::temp_dir().join(format!("procraft-own-brushes-{count}-{free:?}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let s = CanvasSession::new(2000, 1200);
    s.use_gpu(gpu.clone());
    // their grains kept in files beside the library, as the app keeps them
    s.keep_brushes(dir.to_string_lossy().into_owned());
    let file = set_of_own_brushes(&s, count, side);
    let set = s.import_brushes(file).expect("the set brought in");
    let brushes: Vec<u32> = s.brush_library()[set as usize].brushes.iter().map(|b| b.index).collect();
    assert_eq!(brushes.len(), count);
    let _ = gpu.wait();
    let before = reserved(gpu);
    let peak = std::sync::Arc::new(AtomicU64::new(before));
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let sampler = {
        let (gpu, peak, stop) = (gpu.clone(), peak.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(reserved(&gpu), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        })
    };
    crate::device::MEMORY_FREE.with(|m| m.set(free));
    let mb = |v: u64| v.saturating_sub(before) >> 20;
    let started = std::time::Instant::now();
    let (mut drawn, mut first_peak, mut longest_stroke, mut longest_piece, mut full_kept) = (0, 0, 0, 0, 0);
    for (k, &index) in brushes.iter().enumerate() {
        let one = std::time::Instant::now();
        if s.brush_preview(set, index, 530, 162).is_some() {
            drawn += 1;
        }
        longest_stroke = longest_stroke.max(one.elapsed().as_millis());
        if let Some((canvas, _)) = s.shared.lock().preview.as_mut() {
            longest_piece = longest_piece.max(canvas.take_paced_longest().as_millis());
            full_kept = full_kept.max(canvas.full_image_bytes() >> 10);
        }
        assert!(!gpu.is_lost(), "the device lost at brush {k}");
        if k + 1 == count / 10 {
            first_peak = mb(peak.load(Ordering::Relaxed));
        }
    }
    let total = started.elapsed();
    crate::device::MEMORY_FREE.with(|m| m.set(None));
    let at_end = peak.load(Ordering::Relaxed);
    // the library closed
    s.let_go_of_previews();
    let _ = gpu.wait();
    std::thread::sleep(std::time::Duration::from_millis(20));
    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    let after = mb(reserved(gpu));
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
    PreviewRun { drawn, first_peak, peak: mb(at_end), after, longest_stroke, longest_piece, full_kept, total }
}

/// A phone's "graphics stopped" (an Adreno 610, as many brushes brought in drew their sample
/// strokes): 300 brushes of their own brought in at once, each with its own shape and grain, and
/// every one's sample stroke drawn as the library's rows ask for them, on a GPU held to wgpu's
/// downlevel limits, short of memory (a 4 GB phone with a canvas open; the strokes' canvas let go
/// after each), then 150 with memory to spare (the canvas kept between them). The device is never
/// lost and nothing errs; the GPU's memory stays bounded however many are drawn (no more after all
/// than after the first tenth, give or take a few MB, and under a fixed bound), and goes as the
/// library closes; the canvas kept keeps no brush's own images for the next (it kept the last
/// few, 2 MB of these, and a brush engine's set of shapes can be hundreds); no piece of a stroke
/// runs on the GPU near what a driver stops (two seconds). Measured alone on an NVIDIA laptop GPU:
/// 136 to 140 MB at most short of memory, 132 MB with memory to spare, 0 to 12 MB after (blocks
/// the allocator keeps); strokes of 280 and 170 ms at most, their pieces 9 ms (635 ms with every
/// other test running). The numbers are printed.
#[test]
fn hundreds_of_brushes_brought_in_draw_their_strokes_in_bounded_memory_and_time() {
    let gpu = match Gpu::headless_limited(wgpu::Limits::downlevel_defaults()) {
        Ok(gpu) => gpu,
        Err(e) => return eprintln!("skipped: {e}"),
    };
    for (name, count, free) in [("short of memory", 300, Some(600 << 20)), ("memory to spare", 150, None)] {
        let run = previews_of_many_brushes(&gpu, count, 512, free);
        eprintln!(
            "{name}: {} of {count} drawn in {:.1} s; the GPU {} MB over at most after the first tenth, {} MB after \
             all, {} MB after the library closed; the longest stroke {} ms, the longest piece on the GPU {} ms; \
             brushes' own images kept between strokes {} KB at most",
            run.drawn,
            run.total.as_secs_f32(),
            run.first_peak,
            run.peak,
            run.after,
            run.longest_stroke,
            run.longest_piece,
            run.full_kept
        );
        assert!(!gpu.is_lost());
        assert_eq!(gpu.errors(), 0, "GPU errors");
        assert!(run.drawn * 10 >= count * 9, "{name}: {} of {count} drawn", run.drawn);
        assert!(run.peak <= run.first_peak + 16, "{name}: grew from {} MB to {} MB", run.first_peak, run.peak);
        assert!(run.peak <= 200, "{name}: {} MB", run.peak);
        // (what the GPU's allocator keeps of the blocks it made, for the next)
        assert!(run.after <= 16, "{name}: {} MB kept after the library closed", run.after);
        assert_eq!(run.full_kept, 0, "{name}: a brush's own images kept for the next brush's stroke");
        assert!(run.longest_piece < 2000, "{name}: a piece of a stroke took {} ms on the GPU", run.longest_piece);
    }
}
