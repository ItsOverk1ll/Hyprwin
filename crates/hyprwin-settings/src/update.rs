//! "Check for updates": asks GitHub for the newest release. Uses the curl
//! that ships with Windows, so the settings app needs no HTTP library.

use std::os::windows::process::CommandExt;
use std::sync::mpsc::{Receiver, channel};

pub const RELEASES_PAGE: &str = "https://github.com/ItsOverk1ll/Hyprwin/releases";
const LATEST_API: &str = "https://api.github.com/repos/ItsOverk1ll/Hyprwin/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    UpToDate(String),
    Available { version: String, url: String },
    Failed(String),
}

/// Parses "v1.2.3" / "1.2.3" into comparable numbers.
fn parse_version(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

fn check_now() -> Outcome {
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-m", "8", "-H", "User-Agent: hyprwin", "-H", "Accept: application/vnd.github+json", LATEST_API])
        .creation_flags(0x0800_0000)
        .output();
    let Ok(out) = out else { return Outcome::Failed("couldn't run curl".into()) };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Outcome::Failed("no answer from GitHub (offline?)".into());
    };
    let Some(tag) = v["tag_name"].as_str() else {
        return Outcome::Failed("GitHub didn't list a release (the project may be private)".into());
    };
    let current = env!("CARGO_PKG_VERSION");
    if is_newer(tag, current) {
        Outcome::Available { version: tag.trim_start_matches('v').to_string(), url: v["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string() }
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
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
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
    }
}
