//! Artworks being opened, noted in a file of the app's own until each shows. One whose opening
//! ended the app (it was stopped for memory, or crashed) is noted still when the app next starts,
//! so picking it again tells that rather than opening it at once into the same end: the gallery
//! stays usable, and the artwork can be opened anyway, shared or deleted from there.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The file the marks are kept in, once the app has said where.
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the app keeps the marks: a file of its own, outside the gallery. The log of a run before
/// this one that closed while it opened an artwork is kept apart from now on, to share (see
/// `app_log.rs`).
#[uniffi::export]
pub fn set_open_marks_file(path: String) {
    let file = PathBuf::from(path);
    *FILE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file.clone());
    let this_run = std::process::id();
    let ended: Vec<u32> = read(&file).into_iter().map(|(run, _)| run).filter(|&run| run != this_run).collect();
    crate::app_log::keep_log_of_ended_runs(&ended);
}

/// Whether the artwork in folder `dir` was being opened when the app last ended: its opening
/// began in a run of the app that has since ended, and it never showed.
#[uniffi::export]
pub fn opening_ended_app(dir: String) -> bool {
    file().is_some_and(|file| ended_in(&file, Path::new(&dir), std::process::id()))
}

/// The run (process) whose opening of the artwork in folder `dir` closed the app, if one did: for
/// the app to find how that run ended (Android keeps why each of the app's last runs ended).
#[uniffi::export]
pub fn opening_ended_run(dir: String) -> Option<u32> {
    let name = name_of(Path::new(&dir));
    let this_run = std::process::id();
    read(&file()?).into_iter().find(|(run, n)| *n == name && *run != this_run).map(|(run, _)| run)
}

/// The title of what is asked before an artwork whose opening closed the app opens again.
#[uniffi::export]
pub fn opening_ended_app_title() -> String {
    "ProCraft closed while opening this artwork".to_owned()
}

/// What is asked, by how that run ended: the device short of memory (`low_memory`), when closing
/// other apps may help; else a crash, or not known, when it may close again and its log is wanted.
#[uniffi::export]
pub fn opening_ended_app_said(low_memory: bool) -> String {
    if low_memory {
        "Your device ran low on memory. Closing other apps first may help.".to_owned()
    } else {
        "It may close again if you open it now. Share the log so the problem can be found.".to_owned()
    }
}

/// Android's reason for a run that the system ended short of memory
/// (`ApplicationExitInfo.REASON_LOW_MEMORY`).
const ANDROID_LOW_MEMORY: i32 = 3;

/// Whether a run Android says ended for `reason` (its `ApplicationExitInfo` reason) was ended for
/// want of memory; a crash, one not responding, a signal or anything else isn't.
#[uniffi::export]
pub fn android_run_ended_short_of_memory(reason: i32) -> bool {
    reason == ANDROID_LOW_MEMORY
}

/// Whether the marks in `file` say artwork `dir`'s opening began in a run other than `run`.
fn ended_in(file: &Path, dir: &Path, run: u32) -> bool {
    let name = name_of(dir);
    read(file).iter().any(|(r, n)| *n == name && *r != run)
}

/// An artwork being opened: its mark goes when this is dropped (it showed, its opening failed
/// and was told, or it was let go), so only an opening the app ended in leaves one.
pub(crate) struct Mark {
    file: PathBuf,
    name: String,
}

impl Mark {
    /// The artwork in folder `dir` is being opened by this run of the app (nothing is noted until
    /// the app has said where).
    pub(crate) fn new(dir: &Path) -> Option<Mark> {
        Mark::in_file(file()?, dir, std::process::id())
    }

    /// Notes in `file` that run `run` is opening the artwork in folder `dir`.
    fn in_file(file: PathBuf, dir: &Path, run: u32) -> Option<Mark> {
        let name = name_of(dir);
        if name.is_empty() {
            return None;
        }
        let mut marks = read(&file);
        marks.retain(|(_, n)| *n != name);
        marks.push((run, name.clone()));
        write(&file, &marks);
        Some(Mark { file, name })
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        let mut marks = read(&self.file);
        let before = marks.len();
        marks.retain(|(_, n)| *n != self.name);
        if marks.len() != before {
            write(&self.file, &marks);
        }
    }
}

fn file() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(own) = TEST_FILE.with(|f| f.borrow().clone()) {
        return Some(own);
    }
    FILE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[cfg(test)]
thread_local! {
    /// A test's own marks file, on its thread only (tests run side by side).
    static TEST_FILE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Keeps the marks in `file` for what this thread opens (none: as the app says).
#[cfg(test)]
pub(crate) fn use_file_on_this_thread(file: Option<PathBuf>) {
    TEST_FILE.with(|f| *f.borrow_mut() = file);
}

/// An artwork's folder's name: unique in the gallery, and kept as it moves into or out of a stack.
fn name_of(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The marks: the run of the app (its process) that began each opening, and the artwork's folder.
/// A line that doesn't read is left out.
fn read(file: &Path) -> Vec<(u32, String)> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let (run, name) = line.split_once(' ')?;
            let name = name.trim();
            (!name.is_empty()).then_some((run.parse().ok()?, name.to_owned()))
        })
        .collect()
}

/// Writes the marks whole (a failure only logged: the marks only help).
fn write(file: &Path, marks: &[(u32, String)]) {
    let text: String = marks.iter().map(|(run, name)| format!("{run} {name}\n")).collect();
    let written = crate::project::write_atomically(file, |p| {
        crate::storage::write(p, &text).map_err(|e| crate::project::file_error("noting the artwork being opened", e))
    });
    if let Err(e) = written {
        log::warn!("{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_opening_the_app_ended_in_is_told_when_it_starts_again_and_one_that_showed_is_not() {
        let root = std::env::temp_dir().join(format!("procraft-open-marks-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("opening.txt");
        let (a, b) = (root.join("ArtworkA"), root.join("ArtworkB"));
        let (this_run, next_run) = (7, 8);
        // being opened by this run: not told while it runs
        let mark = Mark::in_file(file.clone(), &a, this_run).unwrap();
        assert!(!ended_in(&file, &a, this_run));
        // the app ended while it opened (no drop): told to the next run, of that artwork only
        std::mem::forget(mark);
        assert!(ended_in(&file, &a, next_run));
        assert!(!ended_in(&file, &b, next_run));
        // opened again anyway, by the next run: its own mark, gone once it shows
        let mark = Mark::in_file(file.clone(), &a, next_run).unwrap();
        assert!(!ended_in(&file, &a, next_run));
        drop(mark);
        assert!(!ended_in(&file, &a, next_run));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
        // lines that don't read are left out
        std::fs::write(&file, "x ArtworkB\n2\n3 ArtworkB\n").unwrap();
        assert_eq!(read(&file), [(3, "ArtworkB".to_owned())]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What is asked about an artwork whose opening closed the app says how that run ended: short
    /// of memory, closing other apps may help; a crash, a run not responding, a signal or a reason
    /// not known, the log is asked for. The run is found by the artwork's mark, none for one that
    /// showed or for this run's own.
    #[test]
    fn what_is_asked_after_an_opening_closed_the_app_follows_how_it_ended() {
        let root = std::env::temp_dir().join(format!("procraft-open-ended-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("opening.txt");
        use_file_on_this_thread(Some(file.clone()));
        let (a, b) = (root.join("ArtworkA"), root.join("ArtworkB"));
        let path = |p: &Path| p.to_string_lossy().into_owned();
        std::fs::write(&file, format!("4242 ArtworkA\n{} ArtworkB\n", std::process::id())).unwrap();
        assert_eq!(opening_ended_run(path(&a)), Some(4242));
        assert_eq!(opening_ended_run(path(&b)), None, "this run's own opening");
        assert_eq!(opening_ended_run(path(&root.join("ArtworkC"))), None);
        use_file_on_this_thread(None);
        let _ = std::fs::remove_dir_all(&root);
        // Android's reasons: 3 low memory; 0 unknown, 1 exit itself, 2 signaled, 4 crash, 5 native
        // crash, 6 not responding, 9 excessive resource use, 10 user requested, 13 other
        let said = |reason: i32| opening_ended_app_said(android_run_ended_short_of_memory(reason));
        let memory = "Your device ran low on memory. Closing other apps first may help.";
        let crashed = "It may close again if you open it now. Share the log so the problem can be found.";
        assert_eq!(said(3), memory);
        for reason in [0, 1, 2, 4, 5, 6, 9, 10, 13] {
            assert_eq!(said(reason), crashed, "reason {reason}");
        }
        // nothing known of how it ended (before Android 11, the desktop, the iPad): the log is asked for
        assert_eq!(opening_ended_app_said(false), crashed);
        assert_eq!(opening_ended_app_title(), "ProCraft closed while opening this artwork");
    }
}
