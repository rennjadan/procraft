//! What one render holds on the GPU while it lays a lot of queued work at once.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use procraft_engine::brush::Rendering;
use procraft_engine::document::Document;
use procraft_engine::geom::Vec2;
use procraft_engine::stroke::Stamp;
use procraft_gpu::{Canvas, Gpu, SecondStroke, StrokeStyle, StrokeTool};

const MB: u64 = 1 << 20;

/// `count` stamps of `radius` along a short arc, the `k`th of a ring round the middle of a canvas
/// `side` square.
fn piece(k: usize, side: f32, radius: f32, count: usize) -> Vec<Stamp> {
    (0..count)
        .map(|i| {
            let a = (k * count + i) as f32 * 0.013;
            let at = Vec2::new(side / 2.0 + side * 0.3 * a.cos(), side / 2.0 + side * 0.3 * a.sin());
            Stamp::round(at, radius, 0.6)
        })
        .collect()
}

/// What laying `pieces` queued pieces of a dual brush's stroke built up by its rule (each part
/// four stamps, each piece queued apart, as a shape's moves or a long stroke's frames queue) in one
/// render that shows nothing came to: the layer, the rule's lists held at once, how many times the
/// render waited for the GPU, and the GPU's memory allocated right after the render was recorded
/// over what it held before (none where the backend doesn't say).
fn laid(gpu: &Gpu, side: u32, pieces: usize, lists_at_once: Option<u64>) -> (Vec<u8>, u64, u64, Option<u64>) {
    let mut doc = Document::new(side, side);
    let mut canvas = Canvas::new(gpu, &doc).unwrap();
    if let Some(bytes) = lists_at_once {
        canvas.set_rule_lists_at_once(bytes);
    }
    canvas.render(gpu, &mut doc, None);
    gpu.wait().unwrap();
    let allocated = |gpu: &Gpu| gpu.device.generate_allocator_report().map(|r| r.total_allocated_bytes);
    let before = allocated(gpu);
    let first =
        StrokeStyle { rendering: Rendering::Capped, ..StrokeStyle::new(StrokeTool::Paint, [0.8, 0.2, 0.1], 1.0) };
    let second = SecondStroke {
        style: StrokeStyle { rendering: Rendering::Gradual, ..StrokeStyle::new(StrokeTool::Paint, [1.0; 3], 1.0) },
        mode: 0,
        threshold: -1.0,
        classic: false,
    };
    canvas.begin_dual_stroke(doc.current(), first, second);
    for k in 0..pieces {
        canvas.add_second_stamps(&piece(k, side as f32, 160.0, 4));
        canvas.add_stamps(&piece(k, side as f32, 200.0, 4));
    }
    canvas.render(gpu, &mut doc, None);
    let grew = before.zip(allocated(gpu)).map(|(b, a)| a.saturating_sub(b));
    gpu.wait().unwrap();
    canvas.end_stroke();
    canvas.render(gpu, &mut doc, None);
    let (waits, held) = canvas.rule_lists_held();
    assert!(!canvas.take_work_left_out());
    (canvas.read_layer(gpu, doc.current()).unwrap(), held, waits, grew)
}

/// A render laying a lot of queued work at once (a time-lapse frame or an autosave reading the
/// canvas after a big brush's shape was dragged about, every move queued; a slow GPU's first
/// stroke, before its pace is known) holds the rule's lists of no more than a few pieces at once,
/// sending what it has recorded as they come to [`RULE_LISTS_AT_ONCE`] and letting the GPU get
/// through the pieces before. It took a list of its own for each part of each piece, all held to
/// the render's end: 754 MB of them for 4000 pieces on a 2048 canvas, twice that with their writes
/// staged, and a tablet whose GPU shares its memory was closed short of it. Laid either way, the
/// stroke comes out the same. Printed: the lists held at once, the GPU's memory right after the
/// render was recorded (where the backend says), and the waits, each way. Bounds: at most 80 MB of
/// lists held (twice the most a render takes before sending, and a piece's own); held to the
/// render's end, the 1200 pieces here held more than that.
///
/// [`RULE_LISTS_AT_ONCE`]: 32 MB
#[test]
fn a_render_laying_much_queued_work_holds_few_lists_at_once() {
    let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
    let (side, pieces) = (1024, 1200);
    let (held_back, held, waits, grew) = laid(&gpu, side, pieces, None);
    let (whole, held_whole, _, grew_whole) = laid(&gpu, side, pieces, Some(u64::MAX));
    println!(
        "{pieces} pieces on a {side} canvas ({:?}): lists held at once {} MB ({waits} waits), the GPU {:?} MB more \
         right after the render; held to the render's end {} MB, the GPU {:?} MB more",
        gpu.backend(),
        held / MB,
        grew.map(|g| g / MB),
        held_whole / MB,
        grew_whole.map(|g| g / MB)
    );
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    assert!(held_back.iter().skip(3).step_by(4).any(|&a| a > 0), "laid nothing");
    assert!(held <= 80 * MB, "{} MB of lists held at once", held / MB);
    assert!(held_whole > 80 * MB, "held to the render's end, the lists came to only {} MB", held_whole / MB);
    let differ = held_back.iter().zip(&whole).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} channels laid otherwise sent in pieces");
}
