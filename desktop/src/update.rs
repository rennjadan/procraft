//! Updates on Windows, from the GitHub releases of the repository this copy was built from
//! (`PROCRAFT_UPDATE_REPO` and its build number `PROCRAFT_BUILD`, given by the build workflow).
//!
//! A moment after the app opens, it asks GitHub for the latest release. If that release is a
//! newer build and this copy is the installed one, it asks once whether to update. Yes: the
//! installer is downloaded, a helper waits for the app to close, installs it (Windows asks to
//! allow it) and opens ProCraft again; the app closes itself as the close button would, the
//! artwork open kept. A copy built without those (a developer's, a portable one run elsewhere)
//! never asks.

use std::sync::atomic::{AtomicBool, Ordering};

/// The update is downloaded and its helper waiting: the app is to close.
static READY: AtomicBool = AtomicBool::new(false);

/// Whether the app should close now for an update (asked once).
pub fn take_ready() -> bool {
    READY.swap(false, Ordering::Relaxed)
}

/// The build this copy is, and where its updates come from.
fn built() -> Option<(u64, &'static str)> {
    let build = option_env!("PROCRAFT_BUILD")?.trim().parse::<u64>().ok()?;
    let repo = option_env!("PROCRAFT_UPDATE_REPO")?.trim();
    (!repo.is_empty() && repo.contains('/')).then_some((build, repo))
}

/// Starts looking for an update, on a thread of its own; `wake` tells the app when to close.
pub fn start(window: std::sync::Arc<winit::window::Window>, wake: winit::event_loop::EventLoopProxy<()>) {
    let Some((build, repo)) = built() else { return };
    if !cfg!(windows) || std::env::var_os("PROCRAFT_BACKGROUND").is_some() {
        return;
    }
    let spawned = std::thread::Builder::new().name("procraft-update".to_owned()).spawn(move || {
        // the app open and settled first
        std::thread::sleep(std::time::Duration::from_secs(6));
        if let Err(e) = look(build, repo, &window, &wake) {
            log::info!("update: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("update: {e}");
    }
}

/// Asks for the latest release; offers it if it is newer; on yes, gets it ready.
fn look(
    build: u64,
    repo: &str,
    window: &winit::window::Window,
    wake: &winit::event_loop::EventLoopProxy<()>,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let folder = exe.parent().ok_or("no folder")?.to_string_lossy().into_owned();
    // the latest release's tag and installer, if this copy is the one installed
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$installed = (Get-ItemProperty -Path 'HKLM:\Software\ProCraft' -Name InstallDir -ErrorAction SilentlyContinue).InstallDir
if (-not $installed -or ($installed.TrimEnd('\') -ne '{folder}'.TrimEnd('\'))) {{ 'NOT-INSTALLED'; exit }}
$r = Invoke-RestMethod -Uri 'https://api.github.com/repos/{repo}/releases/latest' -Headers @{{ 'User-Agent' = 'ProCraft' }} -TimeoutSec 20
$msi = $r.assets | Where-Object {{ $_.name -like '*windows-x64.msi' }} | Select-Object -First 1
if ($msi) {{ "$($r.tag_name) $($msi.browser_download_url)" }}"#,
        folder = folder.replace('\'', "''"),
    );
    let answer = powershell(&script)?;
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line.is_empty() || line == "NOT-INSTALLED" {
        return Ok(());
    }
    let (tag, url) = line.split_once(' ').ok_or("an answer not understood")?;
    let newer = tag.strip_prefix("build-").and_then(|n| n.parse::<u64>().ok()).ok_or("a tag not understood")?;
    if newer <= build {
        return Ok(());
    }
    let yes = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("ProCraft update")
        .set_description(
            "A new version of ProCraft is ready.\n\nInstall it now? ProCraft will save, close, update \
             and open again (Windows will ask to allow the installer).",
        )
        .set_buttons(rfd::MessageButtons::YesNo)
        // over the app, which is full screen
        .set_parent(window)
        .show();
    if yes != rfd::MessageDialogResult::Yes {
        return Ok(());
    }
    // downloaded first, so nothing closes for an update that can't be had
    let msi = std::env::temp_dir().join(format!("procraft-{tag}.msi"));
    let target = msi.to_string_lossy().replace('\'', "''");
    powershell(&format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
         Invoke-WebRequest -Uri '{url}' -OutFile '{target}' -UseBasicParsing -TimeoutSec 600"
    ))?;
    // the helper: waits for the app to close, installs, opens it again
    let pid = std::process::id();
    let exe_path = exe.to_string_lossy().replace('\'', "''");
    let helper = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 500; \
         $p = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/i', '\"{target}\"', '/passive', '/norestart') -Verb RunAs -Wait -PassThru; \
         Remove-Item -LiteralPath '{target}' -ErrorAction SilentlyContinue; \
         Start-Process -FilePath '{exe_path}'"
    );
    spawn_hidden(&helper)?;
    READY.store(true, Ordering::Relaxed);
    let _ = wake.send_event(());
    Ok(())
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a PowerShell script with no window, and gives what it printed.
fn powershell(script: &str) -> Result<String, String> {
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    let out = command.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Starts a PowerShell script with no window that outlives the app.
fn spawn_hidden(script: &str) -> Result<(), String> {
    let mut command = std::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}
