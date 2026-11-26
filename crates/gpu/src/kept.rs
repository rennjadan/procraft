//! The pipelines' compiled form, kept between runs where the driver hands it over (Vulkan): a later
//! start makes them without compiling their shaders again, most of the time a first canvas took to
//! open after the app had been closed.
//!
//! The file is the driver's data and a checksum after it. A start that loads it leaves a mark
//! beside it until the pipelines are made: a driver that crashes on the data then finds the mark
//! at the next start, and the data goes.
//!
//! It is kept for one build of the app (see [`keep_pipelines_for`]), and forgotten once a device
//! is lost (see [`forget_kept_pipelines`]): Mali GPUs lost their Vulkan device again and again
//! after an update, loading what the build before had kept (with what the new one added), and a
//! fresh install, starting without it, drew on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Where the app keeps it (its own folder), and the build of the app it is kept for; none, none
/// kept.
static FOLDER: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// How many times what was kept has been forgotten (see [`forget_kept_pipelines`]): a cache opened
/// before the last time is never kept again.
static FORGOTTEN: AtomicU64 = AtomicU64::new(0);

/// How the files the driver's data is kept in begin (see `wgpu::util::pipeline_cache_key`).
const KEPT_PREFIX: &str = "wgpu_pipeline_cache_";

/// The most a kept file holds: a larger one is let go, and the cache made afresh. The driver
/// keeps what it was given with what is new, so each update with shaders of its own adds to it.
const MOST: u64 = 16 << 20;

/// Keeps the pipelines' compiled form in `dir` (the app's own folder), for devices made from now
/// on.
pub fn keep_pipelines_in(dir: PathBuf) {
    keep_pipelines_for(dir, "");
}

/// [`keep_pipelines_in`] for the app's build `build` (on Android, when it was installed or
/// updated, and the system's build): what another build kept is never loaded, and goes.
pub fn keep_pipelines_for(dir: PathBuf, build: &str) {
    *FOLDER.lock().unwrap_or_else(PoisonError::into_inner) = Some((dir, build.trim().to_owned()));
}

/// Where the pipelines' compiled form is kept, if anywhere, and the build it is kept for.
pub(crate) fn folder() -> Option<(PathBuf, String)> {
    FOLDER.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Forgets the pipelines' compiled form kept so far, after a device was lost (what its driver made
/// of them may be what lost it): the files go, and no device made before now keeps them again.
/// How many files went.
pub fn forget_kept_pipelines() -> usize {
    FORGOTTEN.fetch_add(1, Ordering::Relaxed);
    folder().map_or(0, |(dir, _)| forget_in(&dir))
}

/// Removes every file of kept pipelines in `dir` (with their marks); how many went.
fn forget_in(dir: &Path) -> usize {
    removed(dir, |name| name.starts_with(KEPT_PREFIX))
}

/// Removes the files in `dir` whose names `gone` takes; how many went.
fn removed(dir: &Path, gone: impl Fn(&str) -> bool) -> usize {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let names: Vec<PathBuf> =
        entries.map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(&gone)).collect();
    names.iter().filter(|p| std::fs::remove_file(p).is_ok()).count()
}

/// The name of the file a GPU whose cache key is `key` keeps its pipelines in for the app's build
/// `build` (none told: the key alone).
fn file_name(key: &str, build: &str) -> String {
    if build.is_empty() { key.to_owned() } else { format!("{key}_{:016x}", checksum(build.as_bytes())) }
}

/// A device's pipeline cache and the file it is kept in.
pub(crate) struct Kept {
    pub(crate) cache: wgpu::PipelineCache,
    file: PathBuf,
    /// The checksum of what the file holds, if anything.
    saved: Mutex<Option<u64>>,
    /// How many times what was kept had been forgotten as it was opened (see [`FORGOTTEN`]).
    forgotten: u64,
}

impl Kept {
    /// `device`'s pipeline cache kept in `dir` for the app's build `build`: with what an earlier
    /// run of that build left there, else empty (what other builds kept goes). None where the
    /// device can't hand its pipelines over.
    pub(crate) fn open(device: &wgpu::Device, adapter: &wgpu::Adapter, dir: &Path, build: &str) -> Option<Kept> {
        if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return None;
        }
        let forgotten = FORGOTTEN.load(Ordering::Relaxed);
        let key = wgpu::util::pipeline_cache_key(&adapter.get_info())?;
        let name = file_name(&key, build);
        let file = dir.join(&name);
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("the kept pipelines' folder: {e}");
            return None;
        }
        // (and what came before builds were told apart)
        let others = removed(dir, |n| n.starts_with(&key) && !n.starts_with(&name));
        if others > 0 {
            log::info!("the pipelines kept by another build of the app let go ({others} files)");
        }
        let mark = loading(&file);
        if mark.exists() {
            log::warn!("the kept pipelines stopped the last start before they were made: made afresh");
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_file(&mark);
        }
        let data = read(&file);
        if data.is_some()
            && let Err(e) = std::fs::write(&mark, b"")
        {
            // without the mark a crash on the data would come back each start
            log::warn!("marking the kept pipelines: {e}");
            return None;
        }
        // SAFETY: the data is what `PipelineCache::get_data` gave on an earlier run, whole (its
        // checksum checked), in a file named for the adapter (wgpu also checks the adapter and
        // driver it came from, and starts empty when they differ). A driver that crashes on it
        // anyway leaves the mark, and the next start goes without.
        #[allow(unsafe_code)]
        let cache = unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: Some("kept pipelines"),
                data: data.as_ref().map(|(bytes, _)| bytes.as_slice()),
                fallback: true,
            })
        };
        log::info!("the kept pipelines: {} KB", data.as_ref().map_or(0, |(bytes, _)| bytes.len() / 1024));
        Some(Kept { cache, file, saved: Mutex::new(data.map(|(_, sum)| sum)), forgotten })
    }

    /// The pipelines are made with it: it didn't stop the start.
    pub(crate) fn made(&self) {
        let mark = loading(&self.file);
        if mark.exists()
            && let Err(e) = std::fs::remove_file(&mark)
        {
            log::warn!("the kept pipelines' mark: {e}");
        }
    }

    /// Keeps what the cache holds now, unless the file has it already, or what was kept has been
    /// forgotten since it was opened (its device lost, see [`forget_kept_pipelines`]).
    pub(crate) fn save(&self) {
        if self.forgotten != FORGOTTEN.load(Ordering::Relaxed) {
            return;
        }
        let Some(data) = self.cache.get_data() else {
            return;
        };
        let sum = checksum(&data);
        let mut saved = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
        if *saved == Some(sum) {
            return;
        }
        match write(&self.file, &data, sum) {
            Ok(()) => {
                *saved = Some(sum);
                log::info!("the pipelines kept: {} KB", data.len() / 1024);
            }
            Err(e) => log::warn!("keeping the pipelines: {e}"),
        }
    }
}

/// The mark a start leaves while it loads `file`.
fn loading(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".loading");
    PathBuf::from(name)
}

/// What `file` keeps and its checksum, if it is whole.
fn read(file: &Path) -> Option<(Vec<u8>, u64)> {
    let size = std::fs::metadata(file).ok()?.len();
    if size > MOST {
        log::info!("the kept pipelines' file is {size} bytes: made afresh");
        return None;
    }
    let mut bytes = std::fs::read(file).ok()?;
    let at = bytes.len().checked_sub(8)?;
    let tail: [u8; 8] = bytes.get(at..)?.try_into().ok()?;
    bytes.truncate(at);
    let sum = checksum(&bytes);
    if sum != u64::from_le_bytes(tail) {
        log::warn!("the kept pipelines' file is damaged: not used");
        return None;
    }
    Some((bytes, sum))
}

/// Writes `data` and its checksum `sum` to `file` whole: into a file beside it, then put in its
/// place.
fn write(file: &Path, data: &[u8], sum: u64) -> std::io::Result<()> {
    let mut name = file.as_os_str().to_owned();
    name.push(".new");
    let beside = PathBuf::from(name);
    let mut bytes = Vec::with_capacity(data.len() + 8);
    bytes.extend_from_slice(data);
    bytes.extend_from_slice(&sum.to_le_bytes());
    // (one cut short, the storage full, goes: it would only take room)
    std::fs::write(&beside, &bytes).and_then(|()| std::fs::rename(&beside, file)).inspect_err(|_| {
        let _ = std::fs::remove_file(&beside);
    })
}

/// FNV-1a over `bytes`: enough to find a file damaged on the disk.
fn checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builds a phone's app had before and after an update (when it was installed or updated,
    /// and the system's build).
    const BUILD: &str = "1795631556978 samsung/m13dd/m13:14/UP1A.231005.007/E135FXXSEEZE2:user/release-keys";
    const UPDATED: &str = "1795700000000 samsung/m13dd/m13:14/UP1A.231005.007/E135FXXSEEZE2:user/release-keys";

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("procraft-kept-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_kept_file_reads_back_whole_and_a_damaged_one_not_at_all() {
        let dir = folder("file");
        let file = dir.join("cache");
        let data = b"the driver's own bytes".to_vec();
        write(&file, &data, checksum(&data)).unwrap();
        assert_eq!(read(&file), Some((data.clone(), checksum(&data))));
        // nothing left beside it
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let mut damaged = std::fs::read(&file).unwrap();
        damaged[3] ^= 1;
        std::fs::write(&file, &damaged).unwrap();
        assert_eq!(read(&file), None);
        std::fs::write(&file, b"short").unwrap();
        assert_eq!(read(&file), None);
        assert_eq!(read(&dir.join("none")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pipelines_made_again_with_what_a_run_kept_and_a_crash_mark_drops_it() {
        let Ok(gpu) = crate::Gpu::headless() else {
            return;
        };
        let Some(name) = wgpu::util::pipeline_cache_key(&gpu.adapter.get_info()) else {
            return;
        };
        if !gpu.device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return;
        }
        let dir = folder("gpu");
        let file = dir.join(file_name(&name, BUILD));
        let first = Kept::open(&gpu.device, &gpu.adapter, &dir, BUILD).unwrap();
        let _ =
            crate::canvas::Pipelines::new(&gpu.device, crate::canvas::Abilities::of(&gpu), Some(first.cache.clone()));
        first.made();
        first.save();
        let (kept, _) = read(&file).expect("the pipelines kept");
        assert!(!kept.is_empty());
        // a second start loads it, and leaves the mark until its pipelines are made
        let second = Kept::open(&gpu.device, &gpu.adapter, &dir, BUILD).unwrap();
        assert!(loading(&file).exists());
        let _ =
            crate::canvas::Pipelines::new(&gpu.device, crate::canvas::Abilities::of(&gpu), Some(second.cache.clone()));
        second.made();
        assert!(!loading(&file).exists());
        second.save();
        assert!(read(&file).is_some());
        // a start that stopped while loading it: the next goes without
        std::fs::write(loading(&file), b"").unwrap();
        let third = Kept::open(&gpu.device, &gpu.adapter, &dir, BUILD).unwrap();
        assert!(!file.exists() && !loading(&file).exists());
        assert_eq!(*third.saved.lock().unwrap(), None);
        third.save();
        assert!(read(&file).is_some());

        // the app updated: what the build before kept isn't loaded, and goes (with a file from
        // before builds were told apart)
        std::fs::write(dir.join(&name), b"from before builds were told apart").unwrap();
        let updated = Kept::open(&gpu.device, &gpu.adapter, &dir, UPDATED).unwrap();
        assert_eq!(*updated.saved.lock().unwrap(), None, "nothing loaded from another build");
        assert!(!file.exists() && !dir.join(&name).exists(), "another build's kept pipelines go");
        let _ =
            crate::canvas::Pipelines::new(&gpu.device, crate::canvas::Abilities::of(&gpu), Some(updated.cache.clone()));
        updated.made();

        // a device lost: what was kept goes, and a cache opened before isn't kept again
        let after_update = dir.join(file_name(&name, UPDATED));
        assert!(!after_update.exists());
        FORGOTTEN.fetch_add(1, Ordering::Relaxed);
        updated.save();
        assert!(!after_update.exists(), "a cache opened before its device was lost isn't kept");
        let next = Kept::open(&gpu.device, &gpu.adapter, &dir, UPDATED).unwrap();
        let _ =
            crate::canvas::Pipelines::new(&gpu.device, crate::canvas::Abilities::of(&gpu), Some(next.cache.clone()));
        next.save();
        assert!(read(&after_update).is_some(), "one opened since is");
        std::fs::write(dir.join("unrelated.txt"), b"kept").unwrap();
        assert_eq!(forget_in(&dir), 1);
        assert!(!after_update.exists() && dir.join("unrelated.txt").exists());
        assert_eq!(gpu.errors(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_build_keeps_its_pipelines_in_a_file_of_its_own() {
        let key = "wgpu_pipeline_cache_vulkan_5045_1918988290";
        assert_eq!(file_name(key, ""), key);
        let (a, b) = (file_name(key, BUILD), file_name(key, UPDATED));
        assert!(a.starts_with(key) && b.starts_with(key) && a != b && a != key);
        assert_eq!(a, file_name(key, BUILD));
    }
}
