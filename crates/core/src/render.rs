//! The render thread: owns the window's surface and draws a frame whenever something changed.

use std::sync::Arc;
use std::time::Duration;

use procraft_engine::profile::Profile;
use procraft_gpu::{Gpu, wgpu};

use crate::Shared;
use crate::frame::{configure, configure_with, screen_profile};
use crate::frame_loop::Acquired;

/// The window drawn into: Android's native window, or the iPad app's `CAMetalLayer`.
#[cfg(target_os = "android")]
type Window = ndk::native_window::NativeWindow;
#[cfg(target_os = "ios")]
type Window = crate::layer::Layer;

pub(crate) fn run(shared: Arc<Shared>, window: Window) {
    // a device lost while drawing: the lost canvas let go, a new device made (perhaps on OpenGL ES,
    // see `device::lost`), and the artwork reopened on it from its last save; a canvas Vulkan
    // couldn't make: a device made on OpenGL ES, and the canvas made there
    let mut reopening = false;
    loop {
        let again = match draw_into(&shared, &window, reopening) {
            Ended::Stopped => break,
            Ended::Lost => crate::device::recover(&shared),
            Ended::Failed(e) => crate::device::canvas_failed(&shared, &e) || told(&shared, &e),
        };
        if !again {
            // nothing it held draws again: there is only the window's stop to wait for, the app
            // told if no canvas was made (a device lost again and again as one was)
            let mut st = shared.lock();
            if st.canvas.is_none() && st.error.is_none() {
                st.error = Some(procraft_gpu::GpuError::Lost.to_string());
            }
            while !st.stop {
                shared.wake.wait(&mut st);
            }
            break;
        }
        reopening = true;
    }
    drop(window);
}

/// Why [`draw_into`] ended.
enum Ended {
    /// The session said stop (see [`crate::frame_loop::Ended::Stopped`]).
    Stopped,
    /// The device was lost (see [`crate::frame_loop::Ended::Lost`]), perhaps as the canvas was
    /// made.
    Lost,
    /// The canvas couldn't be made on the device (as the error says), and the app isn't told yet:
    /// Vulkan's, made on OpenGL ES instead where it can be (see `device::lost::canvas_failed`).
    Failed(procraft_gpu::GpuError),
}

impl From<crate::frame_loop::Ended> for Ended {
    fn from(ended: crate::frame_loop::Ended) -> Ended {
        match ended {
            crate::frame_loop::Ended::Stopped => Ended::Stopped,
            crate::frame_loop::Ended::Lost => Ended::Lost,
        }
    }
}

/// The canvas couldn't be made (`e`), and nothing else is to be tried: the app is told. False (the
/// window draws nothing again).
fn told(shared: &Shared, e: &procraft_gpu::GpuError) -> bool {
    shared.tell_canvas_failed(e);
    false
}

/// Draws into `window` until the session stops, its device is lost, or its canvas can't be made on
/// a backend that has another to try ([`Ended::Failed`]): on the session's device, or the app's, or
/// one made for the window (`reopening`: after the session's was lost or refused its canvas, the
/// artwork then opened again on it).
fn draw_into(shared: &Arc<Shared>, window: &Window, reopening: bool) -> Ended {
    // the session's own device lost (perhaps while the app was away): a new one first
    if shared.lock().gpu.as_ref().is_some_and(Gpu::is_lost) {
        return Ended::Lost;
    }
    let own = shared.lock().gpu.as_ref().map(|g| g.instance.clone());
    // the app's device (waited for while it is being made), else one made for this window and
    // kept for every artwork after
    let mut kept = crate::device::kept();
    // one lost holds and draws nothing: this artwork makes a new one, and opens as last saved
    if kept.as_ref().is_some_and(Gpu::is_lost) {
        let count = kept.as_ref().map_or(0, crate::device::lost);
        log::warn!("the GPU device was lost ({count} lost this run): making a new one");
        *kept = None;
    }
    // where to make a device from, if there is none to draw with
    let from = if own.is_none() && kept.is_none() { crate::device::first_backend() } else { 0 };
    // until the canvas first shows, a crash counts against the device's backend
    let mut showing = crate::device::Showing::begin();
    let mut instance = own
        .clone()
        .or_else(|| kept.as_ref().map(|g| g.instance.clone()))
        .or_else(|| Gpu::instance_after(from))
        .unwrap_or_else(Gpu::instance);
    // while a device is still to be made, a backend that can't make the window's surface gives way
    // to the next (one whose driver can draw offscreen but not on a window would otherwise leave
    // the canvas blank)
    let count = Gpu::backend_count().max(1);
    let choosing = own.is_none() && kept.is_none();
    let mut tries = 0;
    let mut surface = loop {
        match crate::window::create_surface(&instance, window) {
            Ok(s) => break s,
            Err(e) => {
                let next =
                    (choosing && tries + 1 < count).then(|| Gpu::instance_after((from + tries + 1) % count)).flatten();
                let Some(next) = next else { return fail(shared, e) };
                log::warn!("the window's surface: {e}; trying the next backend");
                tries += 1;
                instance = next;
            }
        }
    };
    let device = if own.is_some() {
        None
    } else if let Some(gpu) = kept.as_ref() {
        Some(gpu.clone())
    } else {
        // the first instance, from the backend the app starts on, that makes a device for the
        // window (see `Gpu::instance_after`, `device::first_backend`)
        loop {
            let backend = (from + tries) % count;
            if reopening {
                log::warn!("making a new GPU device on {}", Gpu::backend_name(backend));
            }
            crate::device::trying(backend);
            match Gpu::for_surface(instance.clone(), &surface) {
                Ok(gpu) => {
                    crate::device::made_on(backend);
                    showing = crate::device::Showing::begin();
                    *kept = Some(gpu.clone());
                    break Some(gpu);
                }
                Err(e) => {
                    // the next backend that makes the window's surface, if any is left
                    let mut why = e.to_string();
                    let made = loop {
                        tries += 1;
                        let next = if tries < count { Gpu::instance_after((from + tries) % count) } else { None };
                        let Some(next) = next else { break None };
                        log::warn!("{why}; trying the next backend");
                        match crate::window::create_surface(&next, window) {
                            Ok(s) => break Some((next, s)),
                            Err(e) => why = format!("the window's surface: {e}"),
                        }
                    };
                    let Some((next, s)) = made else { return fail(shared, why) };
                    surface = s;
                    instance = next;
                }
            }
        }
    };
    drop(kept);
    // (reopening on one made meanwhile for the session, as an export's)
    let device = device.or_else(|| reopening.then(|| shared.lock().gpu.clone()).flatten());
    match device {
        // the session's canvas let go with its lost device: the artwork reopened from its last save
        // on the new one (which it holds from then on)
        Some(gpu) if reopening => {
            let backend = gpu.backend();
            match shared.reopen_after_loss(gpu) {
                Ok(()) => log::warn!("the artwork reopened from its last save on the new GPU device ({backend:?})"),
                Err(e) => return fail(shared, e.to_string()),
            }
        }
        Some(gpu) => {
            // it holds the session's layers from now on
            shared.lock().gpu.get_or_insert(gpu);
        }
        None => {}
    }
    // (reopened, the canvas is made already, or told)
    if !reopening {
        // a canvas that can't be made isn't told yet: a device lost making it is made anew (the
        // artwork opened on the next as it was), and one Vulkan refuses is made on OpenGL ES, before
        // the app says the device can't draw it
        match shared.make_canvas_untold() {
            Ok(()) => {}
            Err(procraft_gpu::GpuError::Lost) => return Ended::Lost,
            Err(e) => {
                let retried = shared.lock().gpu.as_ref().is_some_and(|gpu| crate::device::canvas_retried(gpu, &e));
                if retried {
                    return Ended::Failed(e);
                }
                shared.tell_canvas_failed(&e);
            }
        }
    }
    let queue = {
        let mut st = shared.lock();
        st.reconfigure = true;
        st.needs_frame = true;
        st.gpu.as_ref().map(|g| (g.queue.clone(), g.clone()))
    };
    let Some((queue, gpu)) = queue else { return Ended::Stopped };

    let mut screen = WindowScreen {
        surface,
        queue,
        gpu,
        caps: None,
        config: None,
        #[cfg(target_os = "android")]
        refreshes: crate::window::Refreshes::start(),
    };
    let mut pipelines_kept = false;
    let ended = crate::frame_loop::draw_loop(shared, &mut screen, || {
        showing.shown();
        if !std::mem::replace(&mut pipelines_kept, true) {
            keep_pipelines(shared);
        }
    });
    // (the window's surface let go before another is made on it)
    drop(screen);
    ended.into()
}

/// The window's surface, as the render loop draws into it: its capabilities asked once (they last
/// as long as it does), how it was last configured, and (on Android 13 and later) the screen's
/// refreshes as the system tells them.
struct WindowScreen {
    surface: wgpu::Surface<'static>,
    queue: wgpu::Queue,
    gpu: Gpu,
    caps: Option<wgpu::SurfaceCapabilities>,
    config: Option<wgpu::SurfaceConfiguration>,
    #[cfg(target_os = "android")]
    refreshes: Option<crate::window::Refreshes>,
}

impl crate::frame_loop::Screen for WindowScreen {
    type Image = wgpu::SurfaceTexture;

    fn configure(&mut self, gpu: &Gpu, size: (u32, u32), wide: bool) -> Option<(wgpu::TextureFormat, Profile)> {
        let caps = self.caps.get_or_insert_with(|| self.surface.get_capabilities(&gpu.adapter));
        self.config = configure_with(gpu, &self.surface, caps, size, wide);
        self.config.as_ref().map(|c| (c.format, screen_profile(c)))
    }

    fn acquire(&mut self) -> Acquired<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Acquired::Ready(f),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => Acquired::Suboptimal(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => Acquired::Outdated,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => Acquired::Unavailable,
        }
    }

    fn view(&self, image: &wgpu::SurfaceTexture) -> wgpu::TextureView {
        image.texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    fn present(&mut self, image: wgpu::SurfaceTexture) {
        self.queue.present(image);
    }

    #[cfg(target_os = "android")]
    fn refreshes(&mut self, drawing: bool) -> Option<crate::pacing::Timeline> {
        self.refreshes.as_ref().and_then(|r| r.timeline(drawing))
    }

    fn gpu_idle(&mut self) -> bool {
        // looked at, never waited on: a wait with a timeout lost the device on a tablet's driver
        // (one without, as the canvas's own, never did); and a wait of no time panicked in wgpu as
        // another thread sent work, stopping the renderer (see `Gpu::idle`)
        self.gpu.idle()
    }
}

/// Keeps the device's pipelines for the next run (see [`Gpu::keep_pipelines`]): the screen's,
/// which a canvas's first frame makes, with the canvases'. On a thread of its own (it writes a
/// file).
fn keep_pipelines(shared: &Shared) {
    let Some(gpu) = shared.lock().gpu.clone() else { return };
    let thread = std::thread::Builder::new().name("procraft-keep-pipelines".to_owned());
    if let Err(e) = thread.spawn(move || gpu.keep_pipelines()) {
        log::warn!("keeping the pipelines: {e}");
    }
}

/// How long the second display waits at most for a new frame of the main window before it looks
/// again (its size or its stop come sooner, as each wakes it).
const PROJECTION_WAIT: Duration = Duration::from_secs(1);

/// Project canvas's thread: the whole canvas fitted and centred on black on a second display,
/// drawn again after each frame of the main window.
pub(crate) fn project(shared: Arc<Shared>, window: Window) {
    // the device comes with the main window
    let instance = loop {
        {
            let st = shared.lock();
            if st.projection_stop {
                return;
            }
            if let Some(gpu) = st.gpu.as_ref() {
                break gpu.instance.clone();
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let surface = match crate::window::create_surface(&instance, &window) {
        Ok(s) => s,
        Err(e) => {
            fail(&shared, e);
            return;
        }
    };
    let Some(queue) = shared.lock().gpu.as_ref().map(|g| g.queue.clone()) else { return };
    let mut config: Option<wgpu::SurfaceConfiguration> = None;
    let mut size = (0, 0);
    let mut wide = false;
    let mut drawn: Option<u64> = None;
    loop {
        let (wanted, frames, wants_wide) = {
            let st = shared.lock();
            if st.projection_stop {
                break;
            }
            (st.projection_size, st.frames, st.wide_colour())
        };
        if wanted != size || config.is_none() || wants_wide != wide {
            size = wanted;
            let st = shared.lock();
            let Some(gpu) = st.gpu.as_ref() else { break };
            config = configure(gpu, &surface, size, wants_wide);
            wide = wants_wide;
            drawn = None;
        }
        let Some(format) = config.as_ref().map(|c| c.format) else {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        };
        if drawn == Some(frames) {
            // until the main window draws a frame (it wakes this), or the display's size or stop
            // come
            let mut st = shared.lock();
            if st.frames == frames && st.projection_size == size && !st.projection_stop {
                let _ = shared.wake.wait_for(&mut st, PROJECTION_WAIT);
            }
            continue;
        }
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                config = None;
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut st = shared.lock();
            if st.projection_size != size {
                continue;
            }
            let screen = config.as_ref().map_or(Profile::Srgb, screen_profile);
            st.draw_fitted(&view, format, size, screen);
        }
        drawn = Some(frames);
        queue.present(frame);
    }
    // the picture goes with the canvas: the display is left black
    if config.is_some()
        && let wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) =
            surface.get_current_texture()
    {
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some(gpu) = shared.lock().gpu.as_ref() {
            let mut encoder =
                gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("projection clear") });
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("projection clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            gpu.queue.submit([encoder.finish()]);
        }
        queue.present(frame);
    }
    drop(surface);
    drop(window);
}

fn fail(shared: &Shared, reason: String) -> Ended {
    log::error!("render thread: {reason}");
    shared.lock().error = Some(reason);
    Ended::Stopped
}
