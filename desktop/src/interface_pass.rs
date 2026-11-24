//! The interface drawn into a frame by egui-wgpu, through a small wrapper of ours. egui-wgpu panics
//! when the copies its meshes go up to the GPU through can't be had (`write_buffer_with` gives
//! none: the device lost, an out-of-memory among them, or a buffer of its own made without
//! memory), and that panic took the whole app down. Here such a frame is left undrawn instead, and
//! [`Misses`] says what the app does next: let go of what it keeps and draw again, or, frame after
//! frame missed, start again as it does on a device lost.

use procraft_gpu::{Gpu, wgpu};

/// Records the interface's `jobs` into `encoder`, over `view` (`load` saying what is there first),
/// and gives the uploads to send before it; none if they couldn't be made (nothing is recorded
/// then, and the frame is to be let go unshown: the window keeps the last one).
pub fn record(
    renderer: &mut egui_wgpu::Renderer,
    gpu: &Gpu,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    jobs: &[egui::ClippedPrimitive],
    screen: &egui_wgpu::ScreenDescriptor,
) -> Option<Vec<wgpu::CommandBuffer>> {
    // a device lost takes nothing more, and egui-wgpu would panic on it
    if gpu.is_lost() && !LOST_UNSEEN.with(std::cell::Cell::get) {
        return None;
    }
    let uploads = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        renderer.update_buffers(&gpu.device, &gpu.queue, encoder, jobs, screen)
    }));
    let uploads = match uploads {
        Ok(uploads) => uploads,
        Err(panic) => {
            log::warn!(
                "the interface's frame couldn't go up to the GPU (lost {}): {}",
                gpu.is_lost(),
                procraft_gpu::panic_message(panic.as_ref())
            );
            return None;
        }
    };
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("interface"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.render(&mut pass.forget_lifetime(), jobs, screen);
    }
    Some(uploads)
}

std::thread_local! {
    /// The device taken as not lost when it is, so that egui-wgpu is asked all the same (and
    /// panics): for the tests, to go the way an out-of-memory that loses no device goes.
    static LOST_UNSEEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// How many frames in a row may miss before the app starts again, as on a device lost: each miss
/// lets go of what is kept first, so by then nothing more is to be had.
pub const MISSES_MOST: u32 = 3;

/// The interface's frames missed in a row (see [`record`]).
#[derive(Default)]
pub struct Misses {
    in_a_row: u32,
}

/// What the app does after a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterFrame {
    /// Drawn (or nothing drawn that could miss): nothing to do.
    Fine,
    /// Missed: let go of what is kept only to be quicker, and draw again soon.
    LetGoAndRetry,
    /// Missed [`MISSES_MOST`] times in a row: start again.
    StartAgain,
}

impl Misses {
    /// A frame was drawn (`drawn`) or missed; what the app does now.
    pub fn frame(&mut self, drawn: bool) -> AfterFrame {
        if drawn {
            self.in_a_row = 0;
            return AfterFrame::Fine;
        }
        self.in_a_row += 1;
        if self.in_a_row >= MISSES_MOST { AfterFrame::StartAgain } else { AfterFrame::LetGoAndRetry }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of the interface: a white square over black, at 1 pixel a point, as egui gives it.
    fn frame(ctx: &egui::Context) -> (Vec<egui::ClippedPrimitive>, egui::TexturesDelta) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(64.0, 64.0))),
            ..egui::RawInput::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            let square = egui::Rect::from_min_size(egui::pos2(16.0, 16.0), egui::vec2(32.0, 32.0));
            ui.painter().rect_filled(square, 0, egui::Color32::WHITE);
        });
        (ctx.tessellate(output.shapes, 1.0), output.textures_delta)
    }

    /// Records a frame of the interface into a new picture; the uploads, the picture and the
    /// commands, or none.
    fn drawn(
        gpu: &Gpu,
        renderer: &mut egui_wgpu::Renderer,
        ctx: &egui::Context,
    ) -> Option<(wgpu::Texture, Vec<wgpu::CommandBuffer>)> {
        let (jobs, mut textures) = frame(ctx);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        textures.clear();
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [64, 64], pixels_per_point: 1.0 };
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        let uploads = record(renderer, gpu, &mut encoder, &view, clear, &jobs, &screen)?;
        Some((target, uploads.into_iter().chain([encoder.finish()]).collect()))
    }

    #[test]
    fn the_interface_draws_and_a_lost_device_misses_its_frame_without_a_panic() {
        let Ok(gpu) = Gpu::headless() else { return eprintln!("skipped: no GPU") };
        let ctx = egui::Context::default();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
        // drawn: the square white, the rest black
        let (target, commands) = drawn(&gpu, &mut renderer, &ctx).unwrap();
        gpu.queue.submit(commands);
        let pixels = procraft_gpu::read_texture(&gpu, &target, 64, 64).unwrap();
        let at = |x: usize, y: usize| pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4].to_vec();
        assert_eq!(at(32, 32), [255, 255, 255, 255]);
        assert_eq!(at(4, 4), [0, 0, 0, 255]);
        // the device lost: egui-wgpu asked all the same panics on it (as it did in the app, the
        // staging buffer for its indices not to be had), and the frame is missed, not the app
        gpu.device.destroy();
        LOST_UNSEEN.with(|l| l.set(true));
        let missed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drawn(&gpu, &mut renderer, &ctx)));
        LOST_UNSEEN.with(|l| l.set(false));
        assert!(matches!(missed, Ok(None)), "the frame missed, without a panic");
        // and with the device known to be lost, egui-wgpu isn't asked at all
        if gpu.is_lost() {
            assert!(drawn(&gpu, &mut renderer, &ctx).is_none());
        }
    }

    #[test]
    fn missed_frames_let_go_and_retry_then_start_again_and_a_drawn_one_starts_the_count_afresh() {
        let mut misses = Misses::default();
        assert_eq!(misses.frame(true), AfterFrame::Fine);
        for _ in 1..MISSES_MOST {
            assert_eq!(misses.frame(false), AfterFrame::LetGoAndRetry);
        }
        // a frame drawn between: counted from none again
        assert_eq!(misses.frame(true), AfterFrame::Fine);
        for _ in 1..MISSES_MOST {
            assert_eq!(misses.frame(false), AfterFrame::LetGoAndRetry);
        }
        assert_eq!(misses.frame(false), AfterFrame::StartAgain);
    }
}
