//! A canvas whose texture copies are drawn, as on PowerVR's OpenGL ES (see `stale.rs` in the crate),
//! on a real GPU (skipped where the machine has none).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use procraft_engine::document::{BlendMode, Document, LayerId};
use procraft_engine::geom::{IRect, Vec2};
use procraft_engine::stroke::Stamp;
use procraft_gpu::{Canvas, Gpu, StrokeStyle, StrokeTool};

const W: u32 = 96;
const H: u32 = 64;

/// A layer of lines: opaque colour on a few diagonals (another set for each `seed`), clear between.
fn lines(seed: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (W * H * 4) as usize];
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % W, i as u32 / W);
        if (x + 3 * y + 5 * seed) % 13 < 2 {
            *px = [(40 * seed) as u8, 20, 200 - (30 * seed) as u8, 255];
        }
    }
    rgba
}

fn stroke(canvas: &mut Canvas, layer: LayerId, rgb: [f32; 3], stamps: &[Stamp]) {
    canvas.begin_stroke(layer, StrokeStyle::new(StrokeTool::Paint, rgb, 1.0));
    canvas.add_stamps(stamps);
    canvas.end_stroke();
}

fn shown(gpu: &Gpu, doc: &mut Document, canvas: &mut Canvas, pictures: &mut Vec<Vec<u8>>) {
    canvas.render(gpu, doc, None);
    assert_eq!(gpu.errors(), 0, "GPU validation errors");
    pictures.push(canvas.read_composite(gpu).unwrap());
}

/// The picture after each step of the report: an artwork of three layers opened on a device of
/// little memory (lines on the two below, the top one worked on), so the composite is made around a
/// picture of the layers below it; a dot on the top layer; a stroke across; that undone; the middle
/// layer made Multiply (composited through the other texture of its pair).
fn steps(gpu: &Gpu) -> Vec<Vec<u8>> {
    let mut doc = Document::new(W, H);
    let bottom = doc.current();
    let middle = doc.add_layer();
    let top = doc.add_layer();
    assert!(doc.set_current(top));
    // opened as the app opens an artwork: its layers go up into a canvas with a budget (room for a
    // few layers: little memory)
    let mut canvas = Canvas::unfilled(gpu, &doc).unwrap();
    canvas.set_resident_budget(8 * u64::from(W * H * 4));
    let all = Some(IRect::new(0, 0, W as i32, H as i32));
    canvas.upload_layer_painted(gpu, bottom, &lines(1), all).unwrap();
    canvas.upload_layer_painted(gpu, middle, &lines(2), all).unwrap();
    canvas.fill_rest(gpu, &doc);
    let mut pictures = Vec::new();
    shown(gpu, &mut doc, &mut canvas, &mut pictures);
    assert!(canvas.composites_cached(), "made around its caches");
    stroke(&mut canvas, top, [0.1, 0.1, 0.1], &[Stamp::round(Vec2::new(20.0, 20.0), 4.0, 1.0)]);
    shown(gpu, &mut doc, &mut canvas, &mut pictures);
    let across: Vec<Stamp> =
        (0..40).map(|i| Stamp::round(Vec2::new(4.0 + i as f32 * 2.2, 56.0 - i as f32 * 1.2), 3.0, 1.0)).collect();
    stroke(&mut canvas, top, [0.9, 0.2, 0.1], &across);
    shown(gpu, &mut doc, &mut canvas, &mut pictures);
    canvas.undo();
    shown(gpu, &mut doc, &mut canvas, &mut pictures);
    let before = doc.clone();
    doc.set_blend(middle, BlendMode::Multiply);
    canvas.record_document(before);
    shown(gpu, &mut doc, &mut canvas, &mut pictures);
    pictures
}

/// A canvas made around its caches, its copies drawn, shows at each step what it shows with the
/// GPU's own copies, texel for texel: as an artwork opens the whole drawing (no pixel of it clear,
/// as the composite left clear by PowerVR's copy showed), and after a dot, everything outside it
/// as it was.
#[test]
fn a_canvas_made_around_its_caches_shows_the_whole_drawing_with_its_copies_drawn() {
    let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
    // (the GPU's own copies first, whatever the tests run as)
    gpu.draw_copies(false);
    let copied = steps(&gpu);
    assert_eq!(gpu.copies_drawn_count(), 0);
    gpu.draw_copies(true);
    let drawn = steps(&gpu);
    gpu.draw_copies(false);
    let count = gpu.copies_drawn_count();
    eprintln!("{count} copies drawn ({:?})", gpu.adapter.get_info().backend);
    // the picture of the layers below copied in, and the undo step's pixels kept and put back,
    // each copy drawn
    assert!(count >= 3, "copies drawn: {count}");
    assert_eq!(gpu.copies_not_drawn_count(), 0, "copies copied as before");
    for (step, (c, d)) in copied.iter().zip(&drawn).enumerate() {
        assert!(c == d, "step {step}: drawn, not as copied");
    }
    // opened: the background and the lines below, every pixel
    let opened = &drawn[0];
    assert!(opened.chunks(4).all(|p| p[3] == 255), "the composite has clear pixels");
    let at = |x: u32, y: u32| ((y * W + x) * 4) as usize;
    // (a line of the bottom layer's, one of the middle's, and neither)
    assert_eq!(&opened[at(8, 0)..at(8, 0) + 4], &[40, 20, 170, 255]);
    assert_eq!(&opened[at(3, 0)..at(3, 0) + 4], &[80, 20, 140, 255]);
    assert_eq!(&opened[at(0, 0)..at(0, 0) + 4], &[255, 255, 255, 255]);
    // the dot changed nothing outside its box
    let dot = IRect::new(14, 14, 27, 27);
    let mut changed_outside = 0;
    for (i, (a, b)) in opened.chunks(4).zip(drawn[1].chunks(4)).enumerate() {
        let (x, y) = ((i as u32 % W) as i32, (i as u32 / W) as i32);
        let inside = x >= dot.x0 && x < dot.x1 && y >= dot.y0 && y < dot.y1;
        if !inside && a != b {
            changed_outside += 1;
        }
    }
    assert_eq!(changed_outside, 0, "pixels outside the dot changed");
    assert!(drawn[1] != drawn[0], "the dot shows");
    assert_eq!(gpu.errors(), 0);
}
