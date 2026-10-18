//! "Check for updates": asks GitHub for the newest release. Uses the curl
//! that ships with Windows, so the settings app needs no HTTP library.

use std::os::windows::process::CommandExt;
use std::sync::mpsc::{Receiver, channel};

pub const RELEASES_PAGE: &str = "https://github.com/ItsOverk1ll/Hyprwin/releases";
const LATEST_API: &str = "https://api.github.com/repos/ItsOverk1ll/Hyprwin/releases/latest";
/// Test builds also look at pre-releases ("latest" only ever lists stable ones).
const ALL_API: &str = "https://api.github.com/repos/ItsOverk1ll/Hyprwin/releases?per_page=10";

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    UpToDate(String),
    Available { version: String, url: String },
    Failed(String),
}

/// This build's version, e.g. "0.4.0" or "0.4.0-beta.1".
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A test build (a version with a "-beta…" style suffix).
pub fn is_prerelease() -> bool {
    VERSION.contains('-')
}

/// Parses "v1.2.3" / "1.2.3-beta.4" into (release numbers, pre-release
/// numbers). No pre-release part sorts after any pre-release of the same
/// version: 0.4.0-beta.2 < 0.4.0.
fn parse_version(v: &str) -> (Vec<u64>, Option<Vec<u64>>) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (main, pre) = match v.split_once('-') {
        Some((m, p)) => (m, Some(p)),
        None => (v, None),
    };
    let nums = |s: &str| s.split(['.', '-']).filter_map(|p| p.trim_start_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()).collect::<Vec<u64>>();
    (nums(main), pre.map(nums))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (cm, cp) = parse_version(candidate);
    let (um, up) = parse_version(current);
    if cm != um {
        return cm > um;
    }
    match (cp, up) {
        (None, Some(_)) => true,
        (Some(c), Some(u)) => c > u,
        _ => false,
    }
}

fn check_now() -> Outcome {
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-m", "8", "-H", "User-Agent: hyprwin", "-H", "Accept: application/vnd.github+json", if is_prerelease() { ALL_API } else { LATEST_API }])
        .creation_flags(0x0800_0000)
        .output();
    let Ok(out) = out else { return Outcome::Failed("couldn't run curl".into()) };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Outcome::Failed("no answer from GitHub (offline?)".into());
    };
    // A list (test builds): the newest published release, stable or not.
    let v = match v.as_array() {
        Some(list) => match list.iter().filter(|r| !r["draft"].as_bool().unwrap_or(false)).filter(|r| r["tag_name"].is_string()).reduce(|a, b| {
            if is_newer(b["tag_name"].as_str().unwrap_or(""), a["tag_name"].as_str().unwrap_or("")) { b } else { a }
        }) {
            Some(r) => r.clone(),
            None => return Outcome::Failed("GitHub didn't list a release".into()),
        },
        None => v,
    };
    let Some(tag) = v["tag_name"].as_str() else {
        return Outcome::Failed("GitHub didn't list a release (the project may be private)".into());
    };
    let current = VERSION;
    if is_newer(tag, current) {
        // The link is opened with explorer.exe, which also runs local files:
        // only ever this project's release pages.
        let url = v["html_url"].as_str().filter(|u| u.starts_with(RELEASES_PAGE) && !u.contains(['"', ' ', '\n'])).unwrap_or(RELEASES_PAGE);
        Outcome::Available { version: tag.trim_start_matches('v').to_string(), url: url.to_string() }
    } else {
        Outcome::UpToDate(current.to_string())
    }
}

/// Starts a check in the background.
pub fn start() -> Receiver<Outcome> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(check_now());
    });
    rx
}

pub fn open(url: &str) {
    if url.starts_with(RELEASES_PAGE) {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // Test builds: the final release is newer, and an older stable is not.
        assert!(is_newer("v0.4.0", "0.4.0-beta.1"));
        assert!(is_newer("v0.4.0-beta.2", "0.4.0-beta.1"));
        assert!(!is_newer("v0.3.0", "0.4.0-beta.1"));
        assert!(!is_newer("v0.4.0-beta.1", "0.4.0"));
        assert!(is_newer("v0.4.1-beta.1", "0.4.0"));
    }
}
