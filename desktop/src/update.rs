//! Updates on Windows, from the GitHub releases of the repository this copy was built from
//! (`PROCRAFT_UPDATE_REPO` and its build number `PROCRAFT_BUILD`, given by the build workflow,
//! whose builds are Renn's version with the original developer's newest work merged in).
//!
//! The Updates setting (the gallery's + list) says what happens when a newer build is out:
//! - Automatic (as it starts): it is downloaded quietly, then a question asks to restart:
//!   Restart saves, closes, installs and opens ProCraft again; Later leaves it ready in the row.
//! - Ask first: the same question as soon as one is found; Restart downloads it first.
//! - Off: nothing, unless Check for updates is tapped.
//!
//! What it finds shows in that row itself (no window of its own over the app): checking, up to
//! date, ready, downloading, or what went wrong.
//!
//! It looks a moment after the app opens and every few hours while it stays open. Windows asks to
//! allow the installer each time. Only the installed copy updates: a portable one, or one built
//! without those (a developer's), never does.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use winit::event_loop::EventLoopProxy;
use winit::window::Window;

/// What happens when a newer build is out: [`AUTOMATIC`], 1 (ask first) or [`OFF`].
pub const AUTOMATIC: u8 = 0;
pub const OFF: u8 = 2;

/// The Updates setting's name for `mode`.
pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        AUTOMATIC => "Automatic",
        OFF => "Off",
        _ => "Ask first",
    }
}

static MODE: AtomicU8 = AtomicU8::new(AUTOMATIC);
/// The update is downloaded and its installer waiting: the app is to close now.
static READY: AtomicBool = AtomicBool::new(false);
/// A look under way.
static LOOKING: AtomicBool = AtomicBool::new(false);
/// The release whose installer is downloaded (its tag and file), ready to install on a restart.
static WAITING: Mutex<Option<(String, std::path::PathBuf)>> = Mutex::new(None);
/// A restart is to be asked for (a build found or downloaded), once.
static ASK: AtomicBool = AtomicBool::new(false);
/// The window the questions show over, and how to wake the app to close.
static APP: OnceLock<(Arc<Window>, EventLoopProxy<()>)> = OnceLock::new();

/// What the gallery's Check for updates row says.
#[derive(Clone, Debug, PartialEq)]
enum Status {
    Idle,
    Checking,
    UpToDate,
    /// A newer build is out (its tag and installer), not yet downloaded.
    Ready {
        tag: String,
        url: String,
    },
    Downloading,
    /// Downloaded: it installs when ProCraft restarts.
    Waiting,
    NotInstalled,
    Failed(String),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);
/// The original developer's newest work isn't in the latest build (it clashes with Renn's).
static DEVELOPER_WAITING: AtomicBool = AtomicBool::new(false);

fn set(status: Status) {
    if let Ok(mut s) = STATUS.lock() {
        *s = status;
    }
    if let Some((window, _)) = APP.get() {
        window.request_redraw();
    }
}

fn status() -> Status {
    STATUS.lock().map(|s| s.clone()).unwrap_or(Status::Idle)
}

/// What the Check for updates row says beside it, and whether it is still working (frames wanted).
pub fn row_text() -> (String, bool) {
    let developer = if DEVELOPER_WAITING.load(Ordering::Relaxed) { " (developer's changes waiting)" } else { "" };
    match status() {
        Status::Idle if !available() => ("Not available".to_owned(), false),
        Status::Idle => (String::new(), false),
        Status::Checking => ("Checking\u{2026}".to_owned(), true),
        Status::UpToDate => (format!("Up to date{developer}"), false),
        Status::Ready { .. } => ("Ready \u{2014} tap to install".to_owned(), false),
        Status::Downloading => ("Downloading\u{2026}".to_owned(), true),
        Status::Waiting => ("Ready \u{2014} tap to restart and update".to_owned(), false),
        Status::NotInstalled => ("Only the installed copy".to_owned(), false),
        Status::Failed(e) => (format!("Couldn't check: {}", e.chars().take(40).collect::<String>()), false),
    }
}

/// How long between looks while the app stays open.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// The Updates setting (in the gallery's + list), kept between runs ("updates" in the app's
/// folder).
pub fn mode() -> u8 {
    static READ: std::sync::Once = std::sync::Once::new();
    READ.call_once(|| {
        let kept = std::fs::read_to_string(crate::files::app_dir().join("updates")).unwrap_or_default();
        if let Ok(m) = kept.trim().parse::<u8>() {
            MODE.store(m.min(OFF), Ordering::Relaxed);
        }
    });
    MODE.load(Ordering::Relaxed)
}

/// The Updates setting on to the next: Automatic, Ask first, Off.
pub fn next_mode() {
    let next = (mode() + 1) % 3;
    MODE.store(next, Ordering::Relaxed);
    crate::files::keep_text("updates", &format!("{next}\n"));
}

/// What the app is to close for now, said on its curtain as it does (asked once).
pub fn take_ready() -> Option<&'static str> {
    if !READY.swap(false, Ordering::Relaxed) {
        return None;
    }
    let said = *SAYING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    Some(said.unwrap_or("Updating ProCraft\u{2026}"))
}

/// What the app closes for when it isn't an update (the artworks' folder moving, the graphics
/// changing), said on its curtain.
static SAYING: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// Whether this copy can update itself at all.
pub fn available() -> bool {
    cfg!(windows) && built().is_some()
}

/// The build this copy is, and where its updates come from.
fn built() -> Option<(u64, &'static str)> {
    let build = option_env!("PROCRAFT_BUILD")?.trim().parse::<u64>().ok()?;
    let repo = option_env!("PROCRAFT_UPDATE_REPO")?.trim();
    (!repo.is_empty() && repo.contains('/')).then_some((build, repo))
}

/// Starts looking, a moment after the app opens and then every few hours, on a thread of its own.
pub fn start(window: Arc<Window>, wake: EventLoopProxy<()>) {
    let _ = APP.set((window, wake));
    if !available() || std::env::var_os("PROCRAFT_BACKGROUND").is_some() {
        return;
    }
    let spawned = std::thread::Builder::new().name("procraft-update".to_owned()).spawn(|| {
        std::thread::sleep(Duration::from_secs(8));
        loop {
            if mode() != OFF {
                look(false);
            }
            std::thread::sleep(EVERY);
        }
    });
    if let Err(e) = spawned {
        log::warn!("update: {e}");
    }
}

/// Check for updates tapped (the gallery's + list): with a build ready, it is installed now
/// (saved, closed, installed, opened again); else it looks.
pub fn tap() {
    if !available() {
        set(Status::Idle);
        return;
    }
    let ready = matches!(status(), Status::Ready { .. } | Status::Waiting);
    let spawned = std::thread::Builder::new()
        .name("procraft-update-now".to_owned())
        .spawn(move || if ready { install_now() } else { look(true) });
    if let Err(e) = spawned {
        log::warn!("update: {e}");
    }
}

/// Closes the app as its close button would and opens it again (the artworks' folder moved).
pub fn restart() {
    restart_saying("Moving your artworks\u{2026}");
}

/// Closes the app as its close button would and opens it again, `said` on its curtain.
pub fn restart_saying(said: &'static str) {
    let reopened = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| {
            spawn_hidden(&format!(
                "Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500; Start-Process -FilePath '{}'",
                std::process::id(),
                quoted(&exe.to_string_lossy())
            ))
        });
    match (reopened, APP.get()) {
        (Ok(()), Some((_, wake))) if cfg!(windows) => {
            *SAYING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(said);
            READY.store(true, Ordering::Relaxed);
            let _ = wake.send_event(());
        }
        _ => log::info!("update: close and open ProCraft again to use the new artwork folder"),
    }
}

/// What the latest release is.
enum Found {
    /// This copy isn't the installed one.
    NotInstalled,
    Release {
        build: u64,
        tag: String,
        url: String,
        waiting_developer: bool,
    },
}

/// One look: what is found, and what is done about it.
fn look(asked: bool) {
    if LOOKING.swap(true, Ordering::Relaxed) {
        return;
    }
    if asked || !matches!(status(), Status::Waiting) {
        set(Status::Checking);
    }
    if let Err(e) = look_once(asked) {
        log::info!("update: {e}");
        set(Status::Failed(e.lines().next().unwrap_or_default().to_owned()));
    }
    LOOKING.store(false, Ordering::Relaxed);
}

fn look_once(asked: bool) -> Result<(), String> {
    let Some((build, repo)) = built() else { return Ok(()) };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let folder = exe.parent().ok_or("no folder")?.to_string_lossy().into_owned();
    let (newer, tag, url, waiting_developer) = match latest(repo, &folder)? {
        Found::NotInstalled => {
            set(Status::NotInstalled);
            return Ok(());
        }
        Found::Release { build: newer, tag, url, waiting_developer } => (newer, tag, url, waiting_developer),
    };
    DEVELOPER_WAITING.store(waiting_developer, Ordering::Relaxed);
    if newer <= build {
        set(Status::UpToDate);
        return Ok(());
    }
    let _ = exe;
    // already downloaded (Later was answered): ready in the row, not asked again
    let waiting = WAITING.lock().ok().and_then(|w| w.clone());
    if waiting.is_some_and(|(t, _)| t == tag) {
        set(Status::Waiting);
        if asked {
            ASK.store(true, Ordering::Relaxed);
            wake();
        }
        return Ok(());
    }
    // Automatic: downloaded now, then a restart asked for; else asked for at once (Restart then
    // downloads it)
    if !asked && mode() == crate::update::AUTOMATIC {
        set(Status::Downloading);
        let msi = download(&tag, &url)?;
        if let Ok(mut w) = WAITING.lock() {
            *w = Some((tag, msi));
        }
        set(Status::Waiting);
    } else {
        set(Status::Ready { tag, url });
    }
    ASK.store(true, Ordering::Relaxed);
    wake();
    Ok(())
}

/// The build ready installed now: downloaded, then the app saves, closes, installs and opens again.
fn install_now() {
    let msi = match status() {
        Status::Ready { tag, url } => {
            set(Status::Downloading);
            download(&tag, &url)
        }
        Status::Waiting => {
            WAITING.lock().ok().and_then(|w| w.clone()).map(|(_, m)| m).ok_or_else(|| "nothing downloaded".to_owned())
        }
        _ => return,
    };
    let outcome = msi.and_then(|msi| {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        install_after_exit(&msi, &exe)
    });
    match outcome {
        Ok(()) => {
            READY.store(true, Ordering::Relaxed);
            if let Some((_, wake)) = APP.get() {
                let _ = wake.send_event(());
            }
        }
        Err(e) => set(Status::Failed(e)),
    }
}

/// Downloads release `tag`'s installer (`url`) to the temporary folder: where it is.
fn download(tag: &str, url: &str) -> Result<std::path::PathBuf, String> {
    let msi = std::env::temp_dir().join(format!("procraft-{tag}.msi"));
    let target = quoted(&msi.to_string_lossy());
    powershell(&format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
         Invoke-WebRequest -Uri '{}' -OutFile '{target}' -UseBasicParsing -TimeoutSec 900",
        quoted(url)
    ))?;
    Ok(msi)
}

/// Leaves a helper waiting for the app to close to install `msi`, then open `exe` again.
fn install_after_exit(msi: &std::path::Path, exe: &std::path::Path) -> Result<(), String> {
    let target = quoted(&msi.to_string_lossy());
    let helper = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500; \
         Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', '\"{target}\"', '/qn', '/norestart') -Verb RunAs -Wait; \
         Remove-Item -LiteralPath '{target}' -ErrorAction SilentlyContinue; Start-Process -FilePath '{}'",
        quoted(&exe.to_string_lossy()),
        pid = std::process::id(),
    );
    spawn_hidden(&helper)
}

/// Wakes the app to show what changed.
fn wake() {
    if let Some((window, wake)) = APP.get() {
        window.request_redraw();
        let _ = wake.send_event(());
    }
}

/// Asks to restart for an update when one is ready (found, or downloaded): Restart saves,
/// closes, installs and opens ProCraft again; Later leaves it in the gallery's row.
pub fn prompt(ctx: &egui::Context, look: &mut crate::look::Look, alert: &mut crate::alert::Alert) {
    if ASK.swap(false, Ordering::Relaxed) && !alert.is_open() {
        let downloaded = matches!(status(), Status::Waiting);
        alert.ask(crate::alert::Asking {
            title: "Update ready".into(),
            message: if downloaded {
                "Restart ProCraft to install it? Your artwork is saved first.".into()
            } else {
                "A new version is out. Restart ProCraft to download and install it? Your artwork is saved first.".into()
            },
            action: "Restart".into(),
            icon: None,
            destructive: false,
            extra: None,
            first: Some("Later".into()),
            alone: false,
        });
    }
    if let Some(crate::alert::Answer::Action) = alert.show(ctx, look) {
        tap();
    }
}

/// The latest release of `repo`: its build, tag, installer and whether the developer's newest
/// work is still waiting; or that this copy (in `folder`) isn't the installed one.
fn latest(repo: &str, folder: &str) -> Result<Found, String> {
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$installed = (Get-ItemProperty -Path 'HKLM:\Software\ProCraft' -Name InstallDir -ErrorAction SilentlyContinue).InstallDir
if (-not $installed -or ($installed.TrimEnd('\') -ne '{folder}'.TrimEnd('\'))) {{ 'NOT-INSTALLED'; exit }}
$r = Invoke-RestMethod -Uri 'https://api.github.com/repos/{repo}/releases/latest' -Headers @{{ 'User-Agent' = 'ProCraft' }} -TimeoutSec 20
$msi = $r.assets | Where-Object {{ $_.name -like '*windows-x64.msi' }} | Select-Object -First 1
if (-not $msi) {{ 'NO-INSTALLER'; exit }}
"$($r.tag_name) $($msi.browser_download_url)"
if ($r.body -match 'UPSTREAM-CONFLICT') {{ 'DEVELOPER-WAITING' }}"#,
        folder = quoted(folder),
        repo = quoted(repo),
    );
    let answer = powershell(&script)?;
    let lines: Vec<&str> = answer.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    match lines.first().copied() {
        Some("NOT-INSTALLED") => Ok(Found::NotInstalled),
        Some("NO-INSTALLER") | None => Err("the latest release has no installer".to_owned()),
        Some(line) => {
            let (tag, url) = line.split_once(' ').ok_or("an answer not understood")?;
            let build = tag.strip_prefix("build-").and_then(|n| n.parse::<u64>().ok()).ok_or("a tag not understood")?;
            Ok(Found::Release {
                build,
                tag: tag.to_owned(),
                url: url.to_owned(),
                waiting_developer: lines.contains(&"DEVELOPER-WAITING"),
            })
        }
    }
}

/// `text` inside a PowerShell single-quoted string.
fn quoted(text: &str) -> String {
    text.replace('\'', "''")
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn command(script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    command
}

/// Runs a PowerShell script with no window, and gives what it printed.
fn powershell(script: &str) -> Result<String, String> {
    let out = command(script).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Starts a PowerShell script with no window that outlives the app.
fn spawn_hidden(script: &str) -> Result<(), String> {
    command(script).spawn().map(|_| ()).map_err(|e| e.to_string())
}
