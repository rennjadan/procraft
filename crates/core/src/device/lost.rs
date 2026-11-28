//! The app's GPU device lost while drawing (its driver reset it: work it took too long over, or
//! memory it couldn't find), and what the next device is made from.
//!
//! A device lost holds nothing any more, and the app makes a new one (see `render.rs`). Most losses
//! are one-offs, and a new device from the same backend draws on. But a 4 GB phone's Mali lost its
//! Vulkan device a few hundred milliseconds after a canvas opened, and on each new Vulkan device a
//! few seconds into the first stroke: a new Vulkan device only lost itself again. So a Vulkan
//! device lost soon after a canvas was made ([`SOON_AFTER_CANVAS`]), or a second one lost in a run
//! ([`LOSSES_TO_LEAVE`]), sends the rest of the run to OpenGL ES, and the record says so
//! ([`LOST_RECORD`]) for later starts in the same build. Once the app or the system is updated,
//! Vulkan is tried again.
//!
//! A canvas Vulkan couldn't make at all (its driver refusing what it is asked) goes the same way
//! ([`canvas_failed`]): the artwork opens on OpenGL ES before the app is told the device can't
//! draw it. A Tab S6 Lite's Mali-G72 showed no canvas at all where the build before drew one.
// (the render thread finds a device lost: only the tablet apps have one, and the tests)
#![cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use procraft_gpu::{Gpu, GpuError, wgpu};

use super::{RECORD, Stage, built_by, note_in, read, record, write_record, writing};
use crate::Shared;

/// A Vulkan device lost this soon after a canvas was made is taken as lost by its opening.
const SOON_AFTER_CANVAS: Duration = Duration::from_secs(15);

/// The Vulkan devices lost in a run after which the rest of it is on OpenGL ES.
const LOSSES_TO_LEAVE: usize = 2;

/// The file beside the record that says Vulkan lost the app's device in a build ("vulkan lost"),
/// or couldn't make a canvas ([`FAILED_SAYS`]), then the build (as the record's second line).
pub(crate) const LOST_RECORD: &str = "gpu-lost.txt";

/// What [`LOST_RECORD`] says first where Vulkan couldn't make a canvas.
const FAILED_SAYS: &str = "vulkan made no canvas";

/// The most devices a run loses before an artwork whose device was lost isn't reopened on another
/// (it stays as last saved until it is opened again): a GPU that loses every one draws nothing
/// either way, and each try costs a device and its pipelines.
const REOPENED_MOST: usize = 4;

/// What this run's lost devices have come to (see the module's note).
#[derive(Debug, Default)]
struct Losses {
    /// Devices lost, of any backend.
    all: usize,
    /// Vulkan devices lost.
    vulkan: usize,
    /// Whether the run has left Vulkan for OpenGL ES.
    left: bool,
}

static LOSSES: Mutex<Losses> = Mutex::new(Losses { all: 0, vulkan: 0, left: false });

/// When a canvas was last made on the app's device (see [`canvas_made`]).
static CANVAS_MADE: Mutex<Option<Instant>> = Mutex::new(None);

/// What the next device is made from, after one was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterLoss {
    /// The same backend: not Vulkan, or nothing to go to from it.
    Same,
    /// Vulkan again: the first Vulkan device lost this run, not as a canvas opened.
    VulkanAgain,
    /// OpenGL ES (its index in the backends), from now on.
    OpenGl(usize),
}

/// A canvas was made on the app's device just now (an artwork opened).
pub(crate) fn canvas_made() {
    *CANVAS_MADE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
}

/// Whether a device lost at `lost` was lost soon after a canvas made at `made` (none: no canvas
/// made yet).
fn soon_after(made: Option<Instant>, lost: Instant) -> bool {
    made.is_some_and(|made| lost >= made && lost.duration_since(made) <= SOON_AFTER_CANVAS)
}

/// What the next device is made from after one on `backend` was lost (`soon` after a canvas was
/// made), of the backends `order` (`named`: only those `WGPU_BACKEND` names), this run's `losses`
/// counting it.
fn after_loss(
    losses: &mut Losses,
    backend: wgpu::Backend,
    soon: bool,
    order: &[wgpu::Backends],
    named: bool,
) -> AfterLoss {
    losses.all += 1;
    if backend != wgpu::Backend::Vulkan {
        return AfterLoss::Same;
    }
    losses.vulkan += 1;
    let opengl = order.iter().position(|b| *b == wgpu::Backends::GL);
    match opengl {
        Some(opengl) if !named && (soon || losses.vulkan >= LOSSES_TO_LEAVE) => {
            losses.left = true;
            AfterLoss::OpenGl(opengl)
        }
        Some(_) if !named => AfterLoss::VulkanAgain,
        _ => AfterLoss::Same,
    }
}

/// `gpu`, the app's device, has been found lost: counted once whoever finds it (see the module's
/// note), what comes of it said in the log, and the backend the next device is made from decided
/// (see [`super::first_backend`]). How many devices this run has lost.
pub(crate) fn lost(gpu: &Gpu) -> usize {
    let made = *CANVAS_MADE.lock().unwrap_or_else(PoisonError::into_inner);
    let (all, after) = lost_in(&LOSSES, record(), made, gpu);
    if after.is_some() {
        // what its driver made of the kept pipelines may be what lost it: the next device makes
        // them afresh
        let gone = procraft_gpu::forget_kept_pipelines();
        log::warn!("the GPU device was lost: the kept pipelines forgotten ({gone} files)");
    }
    if let Some(AfterLoss::OpenGl(_)) = after {
        // kept even where some of the canvases' pipelines fail on it, as where Vulkan's driver
        // crashed the app: a device lost again and again draws nothing
        procraft_gpu::vulkan_crashed(true);
    }
    all
}

/// The session's device was found lost by its render thread (see `render.rs`): the loss counted
/// (see [`lost`]), the app's own device let go if it is the one lost, and the session's lost
/// canvas and device let go, so that their memory is free before the next device is made and
/// nothing is saved meanwhile (see `Shared::let_go_of_lost_canvas`). Whether the artwork is to be
/// reopened on a new device: not once the run has lost too many ([`REOPENED_MOST`]).
pub(crate) fn recover(shared: &Shared) -> bool {
    let lost_gpu = shared.lock().gpu.clone();
    let count = match lost_gpu {
        Some(gpu) => lost(&gpu),
        None => LOSSES.lock().unwrap_or_else(PoisonError::into_inner).all,
    };
    let_go(shared, count)
}

/// [`recover`] after the loss was counted, `count` lost this run.
fn let_go(shared: &Shared, count: usize) -> bool {
    {
        let mut kept = super::kept();
        if kept.as_ref().is_some_and(Gpu::is_lost) {
            *kept = None;
        }
    }
    if count > REOPENED_MOST {
        log::warn!(
            "the GPU device was lost ({count} lost this run): the artwork stays as last saved until it is opened again"
        );
        return false;
    }
    log::warn!("the GPU device was lost ({count} lost this run): dropping the lost canvas");
    shared.let_go_of_lost_canvas();
    true
}

/// [`lost`] counted in `losses`, with the record in `record` (its folder and build) and a canvas
/// last made at `made`: how many devices this run has lost, and what comes after this one (none
/// if it was counted before).
fn lost_in(
    losses: &Mutex<Losses>,
    record: Option<(PathBuf, String)>,
    made: Option<Instant>,
    gpu: &Gpu,
) -> (usize, Option<AfterLoss>) {
    if !gpu.take_loss() {
        return (losses.lock().unwrap_or_else(PoisonError::into_inner).all, None);
    }
    let at = gpu.lost_at().unwrap_or_else(Instant::now);
    let order = backends_after(record.as_ref());
    let (after, all, vulkan) = {
        let mut losses = losses.lock().unwrap_or_else(PoisonError::into_inner);
        let after = after_loss(&mut losses, gpu.backend(), soon_after(made, at), &order, Gpu::backends_named());
        (after, losses.all, losses.vulkan)
    };
    let when = made.map_or_else(
        || "before a canvas was made".to_owned(),
        |made| format!("{} ms after a canvas was made", at.saturating_duration_since(made).as_millis()),
    );
    match after {
        AfterLoss::Same => {
            log::warn!(
                "the GPU device was lost ({:?}; {all} lost this run; {when}): the next from the same backend",
                gpu.backend()
            );
        }
        AfterLoss::VulkanAgain => {
            log::warn!("the GPU device was lost on Vulkan (the first this run; {when}): the next on Vulkan again");
        }
        AfterLoss::OpenGl(opengl) => {
            log::warn!(
                "the GPU device was lost on Vulkan ({vulkan} lost this run; {when}): OpenGL ES from now on, and in \
                 later starts until the app or the system is updated"
            );
            if let Some((dir, build)) = &record {
                left_in(dir, build, opengl);
            }
        }
    }
    (all, Some(after))
}

/// The backends a device may be made from after one failed, by the record in `record` (its folder
/// and build): not OpenGL ES where its driver has crashed the app (a crash is worse than a loss).
fn backends_after(record: Option<&(PathBuf, String)>) -> Vec<wgpu::Backends> {
    let saved = record.and_then(|(dir, _)| std::fs::read_to_string(dir.join(RECORD)).ok()).unwrap_or_default();
    let opengl_crashed = super::driver_crashes::opengl_crashed(&saved);
    Gpu::backends().into_iter().filter(|b| !(opengl_crashed && *b == wgpu::Backends::GL)).collect()
}

/// Whether a canvas that couldn't be made on `gpu` (`error`) is made on OpenGL ES instead (see
/// [`canvas_failed`]), before the app is told the device can't draw it.
pub(crate) fn canvas_retried(gpu: &Gpu, error: &GpuError) -> bool {
    let left = LOSSES.lock().unwrap_or_else(PoisonError::into_inner).left;
    retried(gpu.backend(), error, &backends_after(record().as_ref()), Gpu::backends_named(), left)
}

/// [`canvas_retried`] for a device on `backend` that failed with `error`, of the backends `order`
/// (`named`: only those `WGPU_BACKEND` names), in a run that has `left` Vulkan already or not: on
/// Vulkan, where OpenGL ES is still to try; not for a canvas larger than the GPU's textures go
/// (OpenGL ES's go no further), nor a device lost making it (its loss is recovered from as any
/// other's, see [`recover`]).
fn retried(backend: wgpu::Backend, error: &GpuError, order: &[wgpu::Backends], named: bool, left: bool) -> bool {
    backend == wgpu::Backend::Vulkan
        && !named
        && !left
        && order.contains(&wgpu::Backends::GL)
        && !matches!(error, GpuError::TooLarge(..) | GpuError::Lost)
}

/// The session's canvas couldn't be made on its Vulkan device (`error`, one [`canvas_retried`]
/// takes): the rest of the run goes to OpenGL ES, as after a device lost as a canvas opened, and
/// so do later starts in this build unless the GPU was only short of memory; the app's device and
/// the session's are let go (the canvas was never made: what was opened is still there to make it
/// from). The render thread then makes a device on OpenGL ES and the canvas on it (see
/// `render.rs`). Whether it is to: false if OpenGL ES isn't to be tried (nothing changed then).
pub(crate) fn canvas_failed(shared: &Shared, error: &GpuError) -> bool {
    let Some(gpu) = shared.lock().gpu.clone() else { return false };
    if !canvas_retried(&gpu, error) || failed_in(&LOSSES, record(), error).is_none() {
        return false;
    }
    drop(gpu);
    leave_vulkan(shared);
    true
}

/// The session leaves its Vulkan device for good (see [`canvas_failed`]): the app's device let go
/// too if it is Vulkan's, the kept pipelines forgotten (what its driver made of them may be what
/// it refused), and OpenGL ES kept from now on even where some of the canvases' pipelines fail on
/// it, as where Vulkan's driver crashed the app (it is the last there is).
fn leave_vulkan(shared: &Shared) {
    let gone = procraft_gpu::forget_kept_pipelines();
    log::warn!("leaving Vulkan: the kept pipelines forgotten ({gone} files)");
    procraft_gpu::vulkan_crashed(true);
    {
        let mut kept = super::kept();
        if kept.as_ref().is_some_and(|k| k.backend() == wgpu::Backend::Vulkan) {
            *kept = None;
        }
    }
    shared.let_go_of_lost_canvas();
}

/// [`canvas_failed`] counted in `losses`, with the record in `record` (its folder and build): the
/// run left for OpenGL ES (its index in the backends), and the record told so for later starts
/// unless the GPU was only short of memory; none if there is no OpenGL ES, or the run left Vulkan
/// already.
fn failed_in(losses: &Mutex<Losses>, record: Option<(PathBuf, String)>, error: &GpuError) -> Option<usize> {
    let opengl = backends_after(record.as_ref()).iter().position(|b| *b == wgpu::Backends::GL)?;
    {
        let mut losses = losses.lock().unwrap_or_else(PoisonError::into_inner);
        if losses.left {
            return None;
        }
        losses.left = true;
    }
    // (memory short once, as a big artwork leaves it, isn't the driver's doing: Vulkan again next
    // start)
    let kept = !matches!(error, GpuError::OutOfMemory(_));
    log::warn!(
        "Vulkan couldn't make the canvas ({error}): OpenGL ES from now on{}",
        if kept { ", and in later starts until the app or the system is updated" } else { " in this run" }
    );
    if kept && let Some((dir, build)) = &record {
        left_saying(dir, build, opengl, FAILED_SAYS);
    }
    Some(opengl)
}

/// The backend this run makes its devices from since its Vulkan devices were lost: OpenGL ES's
/// index in `order`, if the run has left Vulkan.
pub(crate) fn left_vulkan(order: &[wgpu::Backends]) -> Option<usize> {
    let left = LOSSES.lock().unwrap_or_else(PoisonError::into_inner).left;
    left.then(|| order.iter().position(|b| *b == wgpu::Backends::GL)).flatten()
}

/// Records in `dir`, for `build`, that the app left Vulkan for OpenGL ES (index `opengl`) after
/// Vulkan lost its device: the record starts there, and [`LOST_RECORD`] says why.
fn left_in(dir: &Path, build: &str, opengl: usize) {
    left_saying(dir, build, opengl, "vulkan lost");
}

/// [`left_in`], [`LOST_RECORD`] saying `why` Vulkan was left.
fn left_saying(dir: &Path, build: &str, opengl: usize, why: &str) {
    note_in(dir, build, opengl, Stage::Made);
    let _writing = writing();
    write_record(&dir.join(LOST_RECORD), why, build.trim(), "");
}

/// The backend to start from, of those in `order`, in the build `build`, where the record in `dir`
/// starts from `start` (see `start_in`): OpenGL ES where Vulkan lost the app's device in an
/// earlier run of this build, unless a try on OpenGL ES never finished (it crashed the app, which
/// is worse); Vulkan again where it lost it in another build (the app or the system updated since),
/// unless its driver crashed the app or a try on it never finished.
pub(crate) fn start_for(dir: &Path, build: &str, order: &[wgpu::Backends], start: usize) -> usize {
    let path = dir.join(LOST_RECORD);
    let Ok(lost) = std::fs::read_to_string(&path) else { return start };
    let on = order.get(start).copied();
    let opengl = order.iter().position(|b| *b == wgpu::Backends::GL);
    let saved = std::fs::read_to_string(dir.join(RECORD)).unwrap_or_default();
    let unfinished = matches!(read(&saved), Some((_, Stage::Trying)));
    if built_by(&lost) == build.trim() {
        if on == Some(wgpu::Backends::GL) {
            procraft_gpu::vulkan_crashed(true);
            return start;
        }
        if let (Some(wgpu::Backends::VULKAN), Some(opengl), false) = (on, opengl, unfinished) {
            let what = if lost.starts_with(FAILED_SAYS) { "couldn't make a canvas" } else { "lost the GPU device" };
            log::warn!("Vulkan {what} in an earlier run of this build: starting on OpenGL ES");
            procraft_gpu::vulkan_crashed(true);
            return opengl;
        }
        return start;
    }
    let _ = std::fs::remove_file(&path);
    let vulkan = order.iter().position(|b| *b == wgpu::Backends::VULKAN);
    let held = super::driver_crashes::vulkan_crashed(&saved) || unfinished;
    match vulkan {
        Some(vulkan) if on == Some(wgpu::Backends::GL) && !held => {
            log::info!("the app or the system was updated since Vulkan lost the GPU device: Vulkan tried again");
            vulkan
        }
        _ => start,
    }
}

#[cfg(test)]
mod tests {
    use super::super::start_in;
    use super::*;
    use wgpu::Backends;

    /// Android's backends, Vulkan first.
    const TWO: [Backends; 2] = [Backends::VULKAN, Backends::GL];

    const BUILD: &str = "8011 samsung/m13dd/m13:14/UP1A.231005.007/E135FXXSEEZE2:user/release-keys";
    const HOTFIX: &str = "8012 samsung/m13dd/m13:14/UP1A.231005.007/E135FXXSEEZE2:user/release-keys";

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("procraft-gpu-lost-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The tester's run: a Vulkan device lost 280 ms after a canvas opened goes to OpenGL ES at
    /// once; one lost long after a canvas opened gets a second Vulkan device, and a second loss
    /// goes to OpenGL ES; other backends, or only those named, stay as they are.
    #[test]
    fn a_vulkan_device_lost_as_a_canvas_opens_or_twice_goes_to_opengl() {
        let vulkan = wgpu::Backend::Vulkan;
        let mut losses = Losses::default();
        assert_eq!(after_loss(&mut losses, vulkan, true, &TWO, false), AfterLoss::OpenGl(1));
        assert!(losses.left);

        let mut losses = Losses::default();
        assert_eq!(after_loss(&mut losses, vulkan, false, &TWO, false), AfterLoss::VulkanAgain);
        assert!(!losses.left);
        assert_eq!(after_loss(&mut losses, vulkan, false, &TWO, false), AfterLoss::OpenGl(1));
        assert!(losses.left);
        assert_eq!((losses.all, losses.vulkan), (2, 2));

        // OpenGL ES lost, Direct3D 12 lost: again from the same; Vulkan where there's no OpenGL ES,
        // or WGPU_BACKEND names the backends: the same
        let mut losses = Losses::default();
        assert_eq!(after_loss(&mut losses, wgpu::Backend::Gl, true, &TWO, false), AfterLoss::Same);
        assert_eq!(after_loss(&mut losses, wgpu::Backend::Dx12, true, &TWO, false), AfterLoss::Same);
        assert_eq!(after_loss(&mut losses, vulkan, true, &[Backends::VULKAN], false), AfterLoss::Same);
        assert_eq!(after_loss(&mut losses, vulkan, true, &TWO, true), AfterLoss::Same);
        assert!(!losses.left);
        assert_eq!((losses.all, losses.vulkan), (4, 2));
    }

    #[test]
    fn soon_after_a_canvas_is_within_its_first_seconds() {
        let made = Instant::now();
        assert!(soon_after(Some(made), made + Duration::from_millis(280)));
        assert!(soon_after(Some(made), made + SOON_AFTER_CANVAS));
        assert!(!soon_after(Some(made), made + SOON_AFTER_CANVAS + Duration::from_millis(1)));
        assert!(!soon_after(None, made));
        // lost before the canvas was made (told late): not its opening's doing
        assert!(!soon_after(Some(made + Duration::from_secs(1)), made));
    }

    /// Left for OpenGL ES, the next starts in the build begin there; once the app is updated
    /// Vulkan is tried again, and the record of the loss goes.
    #[test]
    fn a_build_whose_vulkan_lost_the_device_starts_on_opengl_until_updated() {
        let dir = folder("record");
        // drawing on Vulkan, then lost
        note_in(&dir, BUILD, 0, Stage::Trying);
        note_in(&dir, BUILD, 0, Stage::Made);
        note_in(&dir, BUILD, 0, Stage::Drew);
        assert_eq!(start_for(&dir, BUILD, &TWO, start_in(&dir, &TWO)), 0);
        left_in(&dir, BUILD, 1);
        assert_eq!(std::fs::read_to_string(dir.join(RECORD)).unwrap(), format!("1 made\n{BUILD}"));
        assert_eq!(std::fs::read_to_string(dir.join(LOST_RECORD)).unwrap(), format!("vulkan lost\n{BUILD}"));
        assert_eq!(start_for(&dir, BUILD, &TWO, start_in(&dir, &TWO)), 1);
        // it draws there, and stays there in this build
        note_in(&dir, BUILD, 1, Stage::Trying);
        note_in(&dir, BUILD, 1, Stage::Drew);
        assert_eq!(start_for(&dir, BUILD, &TWO, start_in(&dir, &TWO)), 1);
        // even where the record went back to Vulkan meanwhile (written by an older build)
        note_in(&dir, BUILD, 0, Stage::Made);
        assert_eq!(start_for(&dir, BUILD, &TWO, start_in(&dir, &TWO)), 1);
        note_in(&dir, BUILD, 1, Stage::Drew);
        // updated: Vulkan again, the loss forgotten
        assert_eq!(start_for(&dir, HOTFIX, &TWO, start_in(&dir, &TWO)), 0);
        assert!(!dir.join(LOST_RECORD).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Updated, Vulkan isn't tried again where its driver crashed the app or its try never
    /// finished: those keep the app on OpenGL ES for reasons of their own.
    #[test]
    fn an_update_keeps_opengl_where_vulkan_crashed_or_never_finished() {
        let dir = folder("held");
        left_in(&dir, BUILD, 1);
        let crashes = "driver crashes: vulkan 1, gl 0, settled on none, left vulkan yes, seen 5, system s";
        std::fs::write(dir.join(RECORD), format!("1 drew\n{BUILD}\n{crashes}")).unwrap();
        assert_eq!(start_for(&dir, HOTFIX, &TWO, 1), 1);
        assert!(!dir.join(LOST_RECORD).exists());

        left_in(&dir, BUILD, 1);
        std::fs::write(dir.join(RECORD), format!("0 trying\n{HOTFIX}")).unwrap();
        assert_eq!(start_for(&dir, HOTFIX, &TWO, start_in(&dir, &TWO)), 1);

        // in the build Vulkan lost the device in, OpenGL ES crashing the app as it was tried: back
        // to Vulkan (a loss is recovered from, a crash isn't)
        left_in(&dir, BUILD, 1);
        note_in(&dir, BUILD, 1, Stage::Trying);
        assert_eq!(start_in(&dir, &TWO), 0);
        assert_eq!(start_for(&dir, BUILD, &TWO, start_in(&dir, &TWO)), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn opengl_crashes_are_read_from_the_record() {
        use super::super::driver_crashes::opengl_crashed;
        let record = |gl: u32| {
            format!(
                "1 drew\n{BUILD}\ndriver crashes: vulkan 1, gl {gl}, settled on none, left vulkan yes, seen 5, system s"
            )
        };
        assert!(!opengl_crashed(&record(0)));
        assert!(opengl_crashed(&record(2)));
        assert!(!opengl_crashed("0 drew"));
    }

    /// A real device lost on this machine (destroyed, as a driver's reset leaves it) is counted
    /// once however often it is found, and, lost as its canvas opened, the next device is made
    /// from OpenGL ES where there is one, and draws a stroke there.
    #[test]
    fn a_device_lost_as_its_canvas_opens_is_followed_by_one_on_opengl_that_draws() {
        let order = Gpu::backends();
        let Some(opengl) = order.iter().position(|b| *b == Backends::GL) else {
            return eprintln!("skipped: no OpenGL here");
        };
        let Some(vulkan) = order.iter().position(|b| *b == Backends::VULKAN) else {
            return eprintln!("skipped: no Vulkan here");
        };
        let Ok((gpu, _)) = Gpu::for_canvases_from(vulkan, |_| {}) else { return eprintln!("skipped: no GPU") };
        if gpu.backend() != wgpu::Backend::Vulkan {
            return eprintln!("skipped: no Vulkan device");
        }
        let doc = procraft_engine::document::Document::new(64, 64);
        let canvas = procraft_gpu::Canvas::new(&gpu, &doc).unwrap();
        let made = Some(Instant::now());
        gpu.device.destroy();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.wait()));
        assert!(gpu.is_lost());
        let at = gpu.lost_at().expect("the loss's time");
        assert!(gpu.take_loss());
        assert!(!gpu.clone().take_loss(), "a loss is counted once");
        let mut losses = Losses::default();
        let after = after_loss(&mut losses, gpu.backend(), soon_after(made, at), &order, false);
        assert_eq!(after, AfterLoss::OpenGl(opengl));
        drop(canvas);
        drop(gpu);

        let started = Instant::now();
        let Ok((gl, from)) = Gpu::for_canvases_from(opengl, |_| {}) else {
            return eprintln!("skipped: OpenGL makes no device here");
        };
        if from != opengl {
            return eprintln!("skipped: OpenGL's pipelines failed here");
        }
        assert_eq!(gl.backend(), wgpu::Backend::Gl);
        let mut doc = procraft_engine::document::Document::new(64, 64);
        let mut canvas = procraft_gpu::Canvas::new(&gl, &doc).unwrap();
        let layer = doc.current();
        canvas
            .begin_stroke(layer, procraft_gpu::StrokeStyle::new(procraft_gpu::StrokeTool::Paint, [1.0, 0.0, 0.0], 1.0));
        let stamps: Vec<_> = (0..24)
            .map(|i| {
                procraft_engine::stroke::Stamp::round(
                    procraft_engine::geom::Vec2::new(10.0 + 2.0 * i as f32, 32.0),
                    5.0,
                    1.0,
                )
            })
            .collect();
        canvas.add_stamps(&stamps);
        canvas.end_stroke();
        canvas.render(&gl, &mut doc, None);
        let pixels = canvas.read_layer(&gl, layer).unwrap();
        let red = pixels.as_chunks::<4>().0.iter().filter(|p| p[0] > 200 && p[3] > 200).count();
        eprintln!("OpenGL device and stroke in {} ms: {red} red pixels", started.elapsed().as_millis());
        assert!(red > 300, "the stroke drew {red} pixels on OpenGL");
        assert_eq!(gl.errors(), 0);
    }

    /// Every file under `dir` with its bytes, in order.
    fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(at) = todo.pop() {
            for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    todo.push(path);
                } else {
                    found.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
                }
            }
        }
        found.sort();
        found
    }

    /// A stroke across the canvas at height `y`.
    fn stroke(s: &crate::CanvasSession, y: f32) {
        use crate::tests::sample;
        s.stroke_begin(crate::PointerKind::Finger, vec![sample(8.0, y, 0.0)]);
        s.stroke_add((1..6).map(|i| sample(8.0 + i as f32 * 8.0, y, f64::from(i) * 8.0)).collect());
        s.stroke_end(vec![]);
    }

    /// Every layer's pixels, as the canvas has them.
    fn contents(s: &crate::CanvasSession) -> Vec<Vec<u8>> {
        let _ = crate::tests::pixel(s, 0, 0);
        s.layers().into_iter().map(|l| s.copy_layer_contents(l.id).map(|c| c.rgba).unwrap_or_default()).collect()
    }

    /// The session's device lost, as a driver's reset loses it.
    fn lose(s: &crate::CanvasSession) -> Gpu {
        let gpu = s.shared.lock().gpu.clone().unwrap();
        gpu.device.destroy();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.wait()));
        assert!(gpu.is_lost());
        gpu
    }

    /// The tester's run end to end, as the render thread goes through it (see `render.rs`): an
    /// artwork saved and painted on since, and its Vulkan device lost long after it opened. The
    /// loss is counted once; nothing is written over the last save; the lost canvas and device are
    /// let go; a new Vulkan device is made (the first loss this run); and the artwork is reopened on
    /// it as last saved, drawn on and saved. Lost again: the next device is made on OpenGL ES, the
    /// artwork reopened there as last saved, drawn on and saved, and opened from its folder with
    /// what was drawn there. Later starts in the build begin on OpenGL ES.
    #[test]
    fn an_artwork_whose_vulkan_device_is_lost_comes_back_on_vulkan_then_on_opengl() {
        let order = Gpu::backends();
        let position = |backend| order.iter().position(|b| *b == backend);
        let (Some(vulkan), Some(opengl)) = (position(Backends::VULKAN), position(Backends::GL)) else {
            return eprintln!("skipped: no Vulkan and OpenGL here");
        };
        let Some(s) = crate::tests::session(64, 64) else { return };
        if s.shared.lock().gpu.as_ref().map(Gpu::backend) != Some(wgpu::Backend::Vulkan) {
            return eprintln!("skipped: the session isn't on Vulkan");
        }
        let (dir, record) = (folder("artwork"), folder("start-record"));
        let path = dir.to_string_lossy().into_owned();
        let losses = Mutex::new(Losses::default());
        let recorded = || Some((record.clone(), BUILD.to_owned()));
        stroke(&s, 16.0);
        s.save(path.clone()).unwrap();
        let saved = contents(&s);
        let kept = files(&dir);
        assert!(saved.iter().any(|px| px.iter().any(|&v| v != 0)), "painted");

        // painted on since; lost long after it opened
        stroke(&s, 40.0);
        let lost = lose(&s);
        assert_eq!(lost_in(&losses, recorded(), None, &lost), (1, Some(AfterLoss::VulkanAgain)));
        assert_eq!(lost_in(&losses, recorded(), None, &lost), (1, None), "counted once");
        drop(lost);
        assert!(let_go(&s.shared, 1));
        {
            let st = s.shared.lock();
            assert!(st.gpu.is_none() && st.canvas.is_none(), "the lost canvas and device let go");
        }
        assert!(s.save(path.clone()).is_err(), "no save without the canvas");
        assert_eq!(files(&dir), kept, "nothing written over the last save");
        let started = Instant::now();
        let Ok((gpu, from)) = Gpu::for_canvases_from(vulkan, |_| {}) else { return eprintln!("skipped: no device") };
        assert_eq!(from, vulkan);
        s.shared.reopen_after_loss(gpu).unwrap();
        let on_vulkan = started.elapsed();
        assert_eq!(contents(&s), saved, "reopened as last saved");
        stroke(&s, 48.0);
        s.save(path.clone()).unwrap();
        let drawn = contents(&s);
        assert_ne!(drawn, saved, "drawn on after");
        let kept = files(&dir);

        // lost again: OpenGL ES from now on
        stroke(&s, 56.0);
        let lost = lose(&s);
        assert_eq!(lost_in(&losses, recorded(), None, &lost), (2, Some(AfterLoss::OpenGl(opengl))));
        assert!(losses.lock().unwrap().left);
        drop(lost);
        assert!(let_go(&s.shared, 2));
        assert_eq!(files(&dir), kept, "nothing written over the last save");
        let started = Instant::now();
        let Ok((gl, from)) = Gpu::for_canvases_from(opengl, |_| {}) else {
            return eprintln!("skipped: OpenGL makes no device here");
        };
        if from != opengl {
            return eprintln!("skipped: OpenGL's pipelines failed here");
        }
        assert_eq!(gl.backend(), wgpu::Backend::Gl);
        s.shared.reopen_after_loss(gl.clone()).unwrap();
        let on_opengl = started.elapsed();
        assert_eq!(contents(&s), drawn, "reopened on OpenGL ES as last saved");
        stroke(&s, 24.0);
        s.save(path.clone()).unwrap();
        let drawn_on_opengl = contents(&s);
        assert_ne!(drawn_on_opengl, drawn, "drawn on OpenGL ES");
        let again = crate::CanvasSession::open(path).unwrap();
        again.use_gpu(gl.clone());
        assert_eq!(contents(&again), drawn_on_opengl, "the folder has what was drawn on OpenGL ES");
        assert_eq!(gl.errors(), 0);
        // later starts in this build begin on OpenGL ES
        assert_eq!(start_for(&record, BUILD, &order, start_in(&record, &order)), opengl);
        assert_eq!(start_for(&record, HOTFIX, &order, start_in(&record, &order)), vulkan);
        eprintln!(
            "lost twice: a new Vulkan device and the artwork reopened in {} ms, an OpenGL ES device (its pipelines \
             compiled) and the artwork reopened in {} ms",
            on_vulkan.as_millis(),
            on_opengl.as_millis()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&record);
    }

    /// Which canvases that couldn't be made are made on OpenGL ES instead: Vulkan's, refused or
    /// short of memory, while OpenGL ES is still to try. Not one too large for the GPU (OpenGL ES's
    /// textures go no further), nor one whose device was lost (recovered from as any loss), nor on
    /// OpenGL ES or another backend, nor where WGPU_BACKEND names the backends, there is no OpenGL
    /// ES, or the run has left Vulkan already.
    #[test]
    fn a_canvas_vulkan_refuses_is_made_on_opengl_where_that_can_help() {
        let refused = GpuError::Failed("Validation Error: texture width".to_owned());
        let short = GpuError::OutOfMemory("no memory for the texture".to_owned());
        let vulkan = wgpu::Backend::Vulkan;
        assert!(retried(vulkan, &refused, &TWO, false, false));
        assert!(retried(vulkan, &short, &TWO, false, false));
        assert!(!retried(vulkan, &GpuError::TooLarge(20000, 20000, 16384), &TWO, false, false));
        assert!(!retried(vulkan, &GpuError::Lost, &TWO, false, false));
        assert!(!retried(wgpu::Backend::Gl, &refused, &TWO, false, false));
        assert!(!retried(wgpu::Backend::Dx12, &refused, &TWO, false, false));
        assert!(!retried(vulkan, &refused, &TWO, true, false));
        assert!(!retried(vulkan, &refused, &[Backends::VULKAN], false, false));
        assert!(!retried(vulkan, &refused, &TWO, false, true));
    }

    /// A canvas Vulkan refused sends the run to OpenGL ES once, and later starts in the build begin
    /// there until the app is updated; one it was only short of memory for sends the run alone (a
    /// big artwork once is no reason to leave Vulkan for good).
    #[test]
    fn a_build_whose_vulkan_refused_a_canvas_starts_on_opengl_until_updated() {
        let order = Gpu::backends();
        let Some(opengl) = order.iter().position(|b| *b == Backends::GL) else {
            return eprintln!("skipped: no OpenGL here");
        };
        let Some(vulkan) = order.iter().position(|b| *b == Backends::VULKAN) else {
            return eprintln!("skipped: no Vulkan here");
        };
        let refused_dir = folder("refused");
        let dir = refused_dir.clone();
        let recorded = || Some((dir.clone(), BUILD.to_owned()));
        let refused = GpuError::Failed("Validation Error".to_owned());
        note_in(&dir, BUILD, vulkan, Stage::Made);
        let losses = Mutex::new(Losses::default());
        assert_eq!(failed_in(&losses, recorded(), &refused), Some(opengl));
        assert!(losses.lock().unwrap().left);
        assert_eq!(failed_in(&losses, recorded(), &refused), None, "left once");
        assert_eq!(std::fs::read_to_string(dir.join(RECORD)).unwrap(), format!("{opengl} made\n{BUILD}"));
        assert_eq!(std::fs::read_to_string(dir.join(LOST_RECORD)).unwrap(), format!("{FAILED_SAYS}\n{BUILD}"));
        assert_eq!(start_for(&dir, BUILD, &order, start_in(&dir, &order)), opengl);
        assert_eq!(start_for(&dir, HOTFIX, &order, start_in(&dir, &order)), vulkan);

        let dir = folder("short");
        let recorded = || Some((dir.clone(), BUILD.to_owned()));
        note_in(&dir, BUILD, vulkan, Stage::Made);
        let losses = Mutex::new(Losses::default());
        let short = GpuError::OutOfMemory("no memory".to_owned());
        assert_eq!(failed_in(&losses, recorded(), &short), Some(opengl));
        assert!(losses.lock().unwrap().left, "this run on OpenGL ES");
        assert!(!dir.join(LOST_RECORD).exists());
        assert_eq!(start_for(&dir, BUILD, &order, start_in(&dir, &order)), vulkan, "the next start on Vulkan");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&refused_dir);
    }

    /// #412 as the render thread goes through it (see `render.rs`), on this machine: an artwork
    /// saved with a stroke is opened on a Vulkan device that refuses its canvas (an error as it is
    /// made, as a driver that can't make it gives). The app isn't told: no error, no canvas, what
    /// was opened kept and nothing written over the save. The run leaves for OpenGL ES, the Vulkan
    /// device is let go, and the artwork's canvas is made on an OpenGL ES device as it was opened,
    /// drawn on and saved, with no GPU error there.
    #[test]
    fn a_canvas_vulkan_refuses_is_made_on_opengl_before_the_app_is_told() {
        let order = Gpu::backends();
        let position = |backend| order.iter().position(|b| *b == backend);
        let (Some(vulkan), Some(opengl)) = (position(Backends::VULKAN), position(Backends::GL)) else {
            return eprintln!("skipped: no Vulkan and OpenGL here");
        };
        let Ok((on_vulkan, from)) = Gpu::for_canvases_from(vulkan, |_| {}) else {
            return eprintln!("skipped: no GPU");
        };
        if from != vulkan {
            return eprintln!("skipped: no Vulkan device");
        }
        let (dir, record) = (folder("refused-artwork"), folder("refused-record"));
        let path = dir.to_string_lossy().into_owned();
        let saved = {
            let s = crate::CanvasSession::new(64, 64);
            s.use_gpu(on_vulkan.clone());
            s.show_in(64, 64);
            stroke(&s, 16.0);
            s.save(path.clone()).unwrap();
            contents(&s)
        };
        let kept = files(&dir);
        assert!(saved.iter().any(|px| px.iter().any(|&v| v != 0)), "painted");

        // opened, its canvas refused on Vulkan
        let s = crate::CanvasSession::open(path.clone()).unwrap();
        s.show_in(64, 64);
        s.shared.lock().gpu = Some(on_vulkan.clone());
        on_vulkan.refuse_canvases(true);
        let made = s.shared.make_canvas_untold();
        on_vulkan.refuse_canvases(false);
        let error = made.expect_err("the canvas refused");
        eprintln!("refused: {error}");
        {
            let st = s.shared.lock();
            assert!(st.canvas.is_none() && st.canvas_failed);
            assert!(st.error.is_none(), "the app isn't told yet");
            assert!(!st.pending_pixels.is_empty(), "what was opened is kept");
        }
        // none made again on it meanwhile (a call that wants the canvas)
        s.shared.lock().ensure_canvas();
        assert!(s.shared.lock().canvas.is_none());
        assert!(retried(on_vulkan.backend(), &error, &order, false, false));

        // the run leaves for OpenGL ES; the session's Vulkan device let go
        let losses = Mutex::new(Losses::default());
        assert_eq!(failed_in(&losses, Some((record.clone(), BUILD.to_owned())), &error), Some(opengl));
        s.shared.let_go_of_lost_canvas();
        assert!(s.shared.lock().gpu.is_none());
        assert_eq!(files(&dir), kept, "nothing written over the save");
        drop(on_vulkan);

        // made on OpenGL ES as it was opened
        let started = Instant::now();
        let Ok((gl, from)) = Gpu::for_canvases_from(opengl, |_| {}) else {
            return eprintln!("skipped: OpenGL makes no device here");
        };
        if from != opengl {
            return eprintln!("skipped: OpenGL's pipelines failed here");
        }
        s.shared.reopen_after_loss(gl.clone()).unwrap();
        eprintln!(
            "the canvas made on OpenGL ES (its device and pipelines made) in {} ms",
            started.elapsed().as_millis()
        );
        {
            let st = s.shared.lock();
            assert!(st.canvas.is_some(), "the canvas made on OpenGL ES");
            assert!(st.error.is_none());
        }
        assert_eq!(s.reopened_after_loss(), 0, "only the device is new: nothing to read again");
        assert_eq!(contents(&s), saved, "as it was opened");
        stroke(&s, 40.0);
        s.save(path.clone()).unwrap();
        assert_ne!(contents(&s), saved, "drawn on OpenGL ES");
        assert_eq!(gl.errors(), 0);
        assert_eq!(start_for(&record, BUILD, &order, start_in(&record, &order)), opengl);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&record);
    }

    /// A stand-in for the window the render loop draws into: an offscreen texture, ready at once.
    struct Offscreen(wgpu::Texture);

    impl crate::frame_loop::Screen for Offscreen {
        type Image = wgpu::Texture;

        fn configure(
            &mut self,
            _gpu: &Gpu,
            _size: (u32, u32),
            _wide: bool,
        ) -> Option<(wgpu::TextureFormat, procraft_engine::profile::Profile)> {
            Some((wgpu::TextureFormat::Rgba8Unorm, procraft_engine::profile::Profile::Srgb))
        }

        fn acquire(&mut self) -> crate::frame_loop::Acquired<wgpu::Texture> {
            crate::frame_loop::Acquired::Ready(self.0.clone())
        }

        fn view(&self, image: &wgpu::Texture) -> wgpu::TextureView {
            image.create_view(&wgpu::TextureViewDescriptor::default())
        }

        fn present(&mut self, _image: wgpu::Texture) {}
    }

    /// The render loop hands back a device lost while the canvas shows, for the render thread to
    /// make a new one: it waited on it until the window went, the canvas dead. Lost while the app
    /// is away, it waits, and hands it back once the app is back. The loss is counted once.
    #[test]
    fn the_render_loop_hands_back_a_device_lost_while_the_canvas_shows() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(s) = crate::tests::session(64, 64) else { return };
        let shared = Arc::clone(&s.shared);
        let gpu = shared.lock().gpu.clone().unwrap();
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        {
            let mut st = shared.lock();
            st.reconfigure = true;
            st.needs_frame = true;
            st.hidden = true;
        }
        let frames = Arc::new(AtomicUsize::new(0));
        let (running, counted) = (Arc::clone(&shared), Arc::clone(&frames));
        let thread = std::thread::spawn(move || {
            let mut screen = Offscreen(texture);
            crate::frame_loop::draw_loop(&running, &mut screen, || {
                counted.fetch_add(1, Ordering::Relaxed);
            })
        });
        gpu.device.destroy();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.wait()));
        assert!(gpu.is_lost());
        std::thread::sleep(Duration::from_millis(150));
        assert!(!thread.is_finished(), "away, the loop waits for the app's return");
        {
            let mut st = shared.lock();
            st.hidden = false;
            st.needs_frame = true;
            shared.wake.notify_all();
        }
        let back = Instant::now();
        while !thread.is_finished() && back.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        let waited = back.elapsed();
        assert!(thread.is_finished(), "the loop still waits on a lost device");
        assert_eq!(thread.join().unwrap(), crate::frame_loop::Ended::Lost);
        eprintln!("handed back {} ms after the app came back", waited.as_millis());
        assert!(waited < Duration::from_secs(2), "handed back after {} ms", waited.as_millis());
        assert_eq!(frames.load(Ordering::Relaxed), 0, "nothing drawn on a lost device");
        assert!(gpu.take_loss());
        assert!(!gpu.take_loss(), "counted once");
    }
}
