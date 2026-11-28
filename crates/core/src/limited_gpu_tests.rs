//! Artworks on GPUs that offer less than most: an old Mali's limits (#412).

use procraft_engine::brush::{Brush, Rendering};
use procraft_gpu::{Gpu, wgpu};

use crate::tests::sample;
use crate::{CanvasSession, PenSample, PointerKind, ToolKind};

/// What a Mali-G72 (Bifrost, the Galaxy Tab S6 Lite's Exynos 9611) gives wgpu on Vulkan 1.1, at the
/// least: textures of 8192 pixels a side and buffers of 1 GB, as a Bifrost Mali-G52's driver of the
/// same years told a user's log (which gave 35 storage buffers a stage, workgroups of 384 and 32 KB
/// shared besides); held lower where old Mali drivers may report less: 8 storage buffers a stage,
/// 4 colour targets, offsets of 256 bytes, and the rest at wgpu's downlevel least (16 textures
/// and samplers a stage, 4 bind groups, 4 storage textures, 128 MB a storage binding, workgroups
/// of 256 with 16 KB shared).
fn mali_g72_limits() -> wgpu::Limits {
    wgpu::Limits {
        max_texture_dimension_1d: 8192,
        max_texture_dimension_2d: 8192,
        max_buffer_size: 1 << 30,
        max_storage_buffers_per_shader_stage: 8,
        max_color_attachments: 4,
        min_uniform_buffer_offset_alignment: 256,
        min_storage_buffer_offset_alignment: 256,
        ..wgpu::Limits::downlevel_defaults()
    }
}

/// Which brushes of the library are of a kind.
type Wanted = Box<dyn Fn(&Brush) -> bool>;

/// The first brush of the library `wanted` takes, by its set and place.
fn library_brush(s: &CanvasSession, wanted: impl Fn(&Brush) -> bool) -> Option<(u32, u32)> {
    let st = s.shared.lock();
    st.library.iter().enumerate().find_map(|(set, entry)| {
        let at = entry.brushes.iter().position(&wanted)?;
        Some((set as u32, at as u32))
    })
}

/// A stroke across the canvas `width` × `height`, a band down at `y` (0..1).
fn stroke(s: &CanvasSession, width: u32, height: u32, y: f32) {
    let at = |k: usize| PenSample {
        pressure: 0.3 + 0.6 * (k as f32 * 0.2).sin().abs(),
        ..sample(
            width as f32 * (0.1 + 0.02 * k as f32),
            height as f32 * (y + 0.03 * (k as f32 * 0.3).sin()),
            k as f64 * 8.0,
        )
    };
    s.stroke_begin(PointerKind::Stylus, vec![at(0)]);
    for k in 1..40 {
        s.stroke_add(vec![at(k)]);
    }
    s.stroke_end(vec![]);
    render(s);
}

/// Draws what is queued, and waits for it.
fn render(s: &CanvasSession) {
    let mut guard = s.shared.lock();
    let st = &mut *guard;
    if let (Some(canvas), Some(gpu)) = (st.canvas.as_mut(), st.gpu.as_ref()) {
        canvas.render_finished(gpu, &mut st.doc);
        let _ = gpu.wait();
    }
}

/// Every layer's pixels, as the canvas has them.
fn contents(s: &CanvasSession) -> Vec<Vec<u8>> {
    render(s);
    s.layers().into_iter().map(|l| s.copy_layer_contents(l.id).map(|c| c.rgba).unwrap_or_default()).collect()
}

/// #412: on a GPU held to a Mali-G72's limits, the canvases' pipelines are made without an error
/// and the compute passes run, as there; and artworks the size of the tablet's screen, of an A4
/// page at 300 dpi and of 4096 pixels square are made, painted on two layers with a dry, a wet, a
/// dual brush built up by its rule, a brush laid in place and a smudge, saved, and opened again
/// (their canvas made as an opened artwork's is) as they were, then painted on: with no GPU error
/// anywhere. The time each took is printed.
#[test]
fn an_artwork_opens_and_paints_on_a_gpu_held_to_an_old_mali_s_limits() {
    let gpu = match Gpu::headless_limited(mali_g72_limits()) {
        Ok(gpu) => gpu,
        Err(e) => return eprintln!("skipped: {e}"),
    };
    let l = gpu.device.limits();
    assert!(l.max_texture_dimension_2d <= 8192 && l.max_storage_buffers_per_shader_stage <= 8);
    assert!(l.max_color_attachments <= 4 && l.max_sampled_textures_per_shader_stage <= 16);
    let started = std::time::Instant::now();
    gpu.prepare_canvases();
    assert_eq!(gpu.errors(), 0, "the canvases' pipelines");
    assert!(gpu.computes(), "the compute passes run, as on the Mali");
    eprintln!("the pipelines made in {} ms", started.elapsed().as_millis());
    let by_rule = |b: &Brush| matches!(b.rendering, Rendering::Gradual | Rendering::Capped);
    let wanted: [(&str, Wanted); 4] = [
        ("dry", Box::new(|b: &Brush| b.secondary.is_none() && !b.mixes_wet() && !b.laid_in_place())),
        ("wet", Box::new(|b: &Brush| b.mixes_wet())),
        (
            "dual by its rule",
            Box::new(move |b: &Brush| {
                !b.mixes_wet() && !b.laid_in_place() && b.secondary.as_deref().is_some_and(|s| by_rule(s) || by_rule(b))
            }),
        ),
        ("laid in place", Box::new(|b: &Brush| b.laid_in_place() && !b.mixes_wet())),
    ];
    for (w, h) in [(2000, 1200), (2480, 3508), (4096, 4096)] {
        let started = std::time::Instant::now();
        let dir = std::env::temp_dir().join(format!("procraft-mali-g72-{w}x{h}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.to_string_lossy().into_owned();
        let s = CanvasSession::new(w, h);
        s.use_gpu(gpu.clone());
        s.show_in(w, h);
        assert!(s.shared.lock().canvas.is_some(), "{w}×{h}: a new canvas");
        s.add_layer();
        for (k, (kind, wanted)) in wanted.iter().enumerate() {
            let Some((set, index)) = library_brush(&s, wanted) else { panic!("no {kind} brush") };
            assert!(s.select_brush(set, index));
            s.set_brush_size(0.3);
            s.set_color(0.2, 0.4, 0.8);
            stroke(&s, w, h, 0.15 + 0.15 * k as f32);
            assert_eq!(gpu.errors(), 0, "{w}×{h}: a {kind} stroke");
        }
        s.set_tool(ToolKind::Smudge);
        stroke(&s, w, h, 0.4);
        assert_eq!(gpu.errors(), 0, "{w}×{h}: a smudge");
        s.save(path.clone()).unwrap();
        let painted = contents(&s);
        assert!(painted.iter().any(|px| px.iter().any(|&v| v != 0)), "{w}×{h}: painted");
        drop(s);

        let opened = CanvasSession::open(path).unwrap();
        opened.use_gpu(gpu.clone());
        opened.show_in(w, h);
        {
            let st = opened.shared.lock();
            assert!(st.canvas.is_some(), "{w}×{h}: opened, its canvas made: {:?}", st.error);
            assert!(st.error.is_none());
        }
        assert_eq!(contents(&opened), painted, "{w}×{h}: opened as saved");
        opened.set_tool(ToolKind::Paint);
        stroke(&opened, w, h, 0.8);
        assert_eq!(gpu.errors(), 0, "{w}×{h}: opened and painted on");
        drop(opened);
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("{w}×{h}: made, painted, saved and opened again in {} ms", started.elapsed().as_millis());
    }
}
