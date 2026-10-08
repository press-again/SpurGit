//! Self-update from the GitHub releases the CI `release` jobs publish.
//! Uses the system `curl` and `tar`/`ditto` (shipped with Windows 10+ and
//! macOS), so no HTTP or zip dependency.

use std::path::{Path, PathBuf};
use std::process::Command;

const LATEST: &str = "https://api.github.com/repos/press-again/SpurGit/releases/latest";
/// Only assets from this repository's releases are ever installed.
const DOWNLOAD_PREFIX: &str = "https://github.com/press-again/SpurGit/releases/download/";

/// Release asset suffix and the file the archive unpacks to, per platform;
/// `None` where CI publishes no build.
#[cfg(all(windows, target_arch = "x86_64"))]
const ASSET: Option<(&str, &str)> = Some(("-windows-x64.zip", "spurgit.exe"));
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ASSET: Option<(&str, &str)> = Some(("-macos-arm64.zip", "Spur.app"));
#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
const ASSET: Option<(&str, &str)> = None;

#[derive(Clone)]
pub struct Release {
    pub version: String,
    url: String,
}

/// Blocking: ask GitHub for the latest release; `Some` when it is newer than
/// this build and has an asset for this platform.
pub fn check() -> Result<Option<Release>, String> {
    let Some((suffix, _)) = ASSET else {
        return Ok(None);
    };
    // Development builds cannot install, so they do not offer updates either.
    let Ok(target) = install_target() else {
        return Ok(None);
    };
    // Leftover from the previous update (a running .exe cannot be deleted).
    let _ = remove(&old_path(&target));
    let body = run(curl().args(["--max-time", "30"]).arg(LATEST))?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(pick(&json, env!("CARGO_PKG_VERSION"), suffix))
}

fn pick(json: &serde_json::Value, current: &str, suffix: &str) -> Option<Release> {
    let tag = json["tag_name"].as_str()?;
    if parse_version(tag) <= parse_version(current) {
        return None;
    }
    let url = json["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix)))?["browser_download_url"]
        .as_str()
        .filter(|url| url.starts_with(DOWNLOAD_PREFIX))?;
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        url: url.to_string(),
    })
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// Blocking: download and unpack the release next to the running app, then
/// swap it in. Returns what to relaunch.
pub fn install(release: &Release) -> Result<PathBuf, String> {
    let (_, packed) = ASSET.ok_or("no release build for this platform")?;
    let target = install_target()?;
    // Stage beside the target so the swap is a same-volume rename.
    let dir = target.parent().ok_or("app has no parent folder")?.join(".spurgit-update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let result = (|| {
        let zip = dir.join("update.zip");
        // Abort a stalled download (under 1 KB/s for 30 s) rather than capping its length.
        run(curl()
            .args(["--speed-limit", "1024", "--speed-time", "30", "-o"])
            .arg(&zip)
            .arg(&release.url))?;
        unpack(&zip, &dir)?;
        let fresh = dir.join(packed);
        if !fresh.exists() {
            return Err(format!("{packed} missing from the release archive"));
        }
        let old = old_path(&target);
        let _ = remove(&old);
        // A running .exe or .app can be renamed, not overwritten.
        std::fs::rename(&target, &old).map_err(|e| format!("{}: {e}", target.display()))?;
        if let Err(e) = std::fs::rename(&fresh, &target) {
            let _ = std::fs::rename(&old, &target);
            return Err(format!("{}: {e}", target.display()));
        }
        // macOS can delete the old bundle now; Windows on next check.
        let _ = remove(&old);
        Ok(target.clone())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Start the freshly installed app; the caller quits this one.
pub fn relaunch(target: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let spawned = Command::new("open").arg("-n").arg(target).spawn();
    #[cfg(not(target_os = "macos"))]
    let spawned = Command::new(target).spawn();
    spawned.map(drop).map_err(|e| format!("restart failed, start Spur again: {e}"))
}

/// The running executable (Windows) or its `.app` bundle (macOS).
fn install_target() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        return Err("development builds do not self-update".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if cfg!(target_os = "macos") {
        // Spur.app/Contents/MacOS/spurgit
        return exe
            .ancestors()
            .nth(3)
            .filter(|app| app.extension().is_some_and(|ext| ext == "app"))
            .map(Path::to_path_buf)
            .ok_or_else(|| "not running from an app bundle".into());
    }
    Ok(exe)
}

fn old_path(target: &Path) -> PathBuf {
    target.with_extension("old")
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn unpack(zip: &Path, dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("ditto");
    #[cfg(target_os = "macos")]
    cmd.args(["-x", "-k"]).arg(zip).arg(dir);
    #[cfg(not(target_os = "macos"))]
    let mut cmd = Command::new("tar");
    #[cfg(not(target_os = "macos"))]
    cmd.arg("-xf").arg(zip).arg("-C").arg(dir);
    run(&mut cmd).map(drop)
}

fn curl() -> Command {
    let mut cmd = Command::new("curl");
    // `--proto =https` also covers redirects to GitHub's asset host.
    cmd.args(["-fsSL", "--proto", "=https", "--connect-timeout", "15"]);
    cmd.args(["-H", "User-Agent: SpurGit"]);
    cmd
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // GUI launches have no console; hide the per-command console flash.
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, asset: &str, url: &str) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "assets": [{ "name": asset, "browser_download_url": url }],
        })
    }

    #[test]
    fn picks_only_newer_matching_assets_from_this_repo() {
        let url = format!("{DOWNLOAD_PREFIX}v1.10.0/spurgit-v1.10.0-macos-arm64.zip");
        let json = release("v1.10.0", "spurgit-v1.10.0-macos-arm64.zip", &url);
        let hit = pick(&json, "1.9.3", "-macos-arm64.zip").expect("newer release");
        assert_eq!(hit.version, "1.10.0");
        assert_eq!(hit.url, url);
        assert!(pick(&json, "1.10.0", "-macos-arm64.zip").is_none());
        assert!(pick(&json, "2.0.0", "-macos-arm64.zip").is_none());
        assert!(pick(&json, "1.9.3", "-windows-x64.zip").is_none());
        let foreign = release("v9.0.0", "x-macos-arm64.zip", "https://evil.example/x.zip");
        assert!(pick(&foreign, "1.0.0", "-macos-arm64.zip").is_none());
    }
}
