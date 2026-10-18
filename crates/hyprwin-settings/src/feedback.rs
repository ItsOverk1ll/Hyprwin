//! Settings › Feedback: send a message (and, if the user agrees, a
//! diagnostic report) to the project's Discord channel, or save it as a file.
//!
//! The Discord webhook is baked in at build time from the
//! `HYPRWIN_FEEDBACK_WEBHOOK` environment variable (a GitHub secret in the
//! release workflow), so it never sits in the public repository. A build
//! without it can still save reports to the Desktop.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, Color32, RichText};
use hyprwin_win32::crash;

const NO_WINDOW: u32 = 0x0800_0000;
/// Shortest gap between two sends (so a stuck button can't flood the channel).
const SEND_GAP: Duration = Duration::from_secs(60);

fn webhook() -> Option<&'static str> {
    option_env!("HYPRWIN_FEEDBACK_WEBHOOK")
        .map(str::trim)
        .filter(|u| u.starts_with("https://discord.com/api/webhooks/") || u.starts_with("https://discordapp.com/api/webhooks/"))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Bug,
    Idea,
    Other,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Bug => "Something isn't working",
            Kind::Idea => "An idea or request",
            Kind::Other => "Something else",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Kind::Bug => "🐞 Bug report",
            Kind::Idea => "💡 Idea",
            Kind::Other => "💬 Feedback",
        }
    }
    fn color(self) -> u32 {
        match self {
            Kind::Bug => 0xF06E6E,
            Kind::Idea => 0x33CCFF,
            Kind::Other => 0x78DC96,
        }
    }
}

pub struct Feedback {
    kind: Kind,
    message: String,
    name: String,
    include_report: bool,
    include_titles: bool,
    sending: Option<Receiver<Result<(), String>>>,
    last_sent: Option<Instant>,
    result: Option<Result<String, String>>,
    preview: Option<String>,
}

impl Default for Feedback {
    fn default() -> Self {
        Feedback {
            kind: Kind::Bug,
            message: String::new(),
            name: String::new(),
            include_report: true,
            include_titles: false,
            sending: None,
            last_sent: None,
            result: None,
            preview: None,
        }
    }
}

// ---------------------------------------------------------------- the report

fn reg_value(key: &str, name: &str) -> Option<String> {
    let out = std::process::Command::new("reg.exe").args(["query", key, "/v", name]).creation_flags(NO_WINDOW).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = text.lines().find(|l| l.trim_start().starts_with(name))?;
    let v = line.split_once("REG_")?.1;
    Some(v.split_once(char::is_whitespace)?.1.trim().to_string())
}

pub fn windows_version() -> String {
    const K: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg_value(K, "CurrentBuild").unwrap_or_default();
    let ubr = reg_value(K, "UBR").and_then(|u| u.strip_prefix("0x").and_then(|h| u64::from_str_radix(h, 16).ok())).map(|n| format!(".{n}")).unwrap_or_default();
    let edition = reg_value(K, "EditionID").unwrap_or_default();
    let display = reg_value(K, "DisplayVersion").unwrap_or_default();
    // ProductName still says "Windows 10" on Windows 11; the build tells them apart.
    let name = if build.parse::<u32>().unwrap_or(0) >= 22000 { "Windows 11" } else { "Windows 10" };
    format!("{name} {edition} {display} (build {build}{ubr})").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces every `q`-quoted part of `line` with `q…q`.
fn hide_quoted(line: &str, q: char) -> String {
    let mut r = String::with_capacity(line.len());
    let mut parts = line.split(q);
    if let Some(first) = parts.next() {
        r.push_str(first);
    }
    for (i, p) in parts.enumerate() {
        r.push(q);
        if i % 2 == 0 {
            r.push('…');
        } else {
            r.push_str(p);
        }
    }
    r
}

/// Log text with window titles hidden (the log quotes them: `'Inbox - Mail'`,
/// `("Untitled - Notepad")`), unless the user chose to include them.
fn redact_titles(text: &str, keep_titles: bool) -> String {
    if keep_titles {
        return text.to_string();
    }
    text.lines().map(|l| hide_quoted(&hide_quoted(l, '\''), '"')).collect::<Vec<_>>().join("\n")
}

/// The user's name in paths becomes `<you>`.
fn hide_user(text: &str) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.len() < 2 {
        return text.to_string();
    }
    text.replace(&format!("\\{user}\\"), "\\<you>\\").replace(&format!("\\{user}"), "\\<you>")
}

#[cfg(test)]
fn redact(text: &str, keep_titles: bool) -> String {
    hide_user(&redact_titles(text, keep_titles))
}

fn ipc(req: &str) -> String {
    hyprwin_ipc::request(req).unwrap_or_else(|_| "(hyprwin isn't running)".into())
}

/// Everything that helps track a problem down, as plain text.
pub fn build_report(keep_titles: bool) -> String {
    let mut r = String::new();
    let line = |r: &mut String, s: String| {
        r.push_str(&s);
        r.push('\n');
    };
    line(&mut r, format!("hyprwin {}", crate::update::VERSION));
    line(&mut r, format!("{}", windows_version()));
    line(&mut r, format!("report made {} UTC", crash::utc_stamp(SystemTime::now())));
    line(&mut r, format!("installed in {}", crate::setup::bin_dir().display()));
    line(&mut r, format!("status: {}", ipc("status")));
    line(&mut r, String::new());
    line(&mut r, "---- monitors ----".into());
    let mons = ipc("j/monitors");
    match serde_json::from_str::<serde_json::Value>(&mons).ok().and_then(|v| v.as_array().cloned()) {
        Some(list) => {
            for m in list {
                line(
                    &mut r,
                    format!(
                        "{}: {}x{} @ {} Hz, scale {}, at {},{}{}",
                        m["name"].as_str().unwrap_or("?"),
                        m["width"],
                        m["height"],
                        m["refreshRate"],
                        m["scale"],
                        m["x"],
                        m["y"],
                        if m["focused"].as_bool() == Some(true) { " (focused)" } else { "" }
                    ),
                );
            }
        }
        None => line(&mut r, mons),
    }
    line(&mut r, "---- config problems ----".into());
    let ce = ipc("configerrors");
    line(&mut r, if ce.trim().is_empty() { "(none)".into() } else { ce });
    let log = crash::log_tail(4000);
    let guard_problem = |l: &str| l.starts_with("guard:") && ["EMERGENCY", "unexpectedly", "WARNING", "cannot"].iter().any(|k| l.contains(k));
    let problems: Vec<&str> = log.lines().filter(|l| l.contains(" WARN ") || l.contains(" ERROR ") || guard_problem(l)).collect();
    line(&mut r, format!("---- problems hyprwin noticed (last {} of {}) ----", problems.len().min(80), problems.len()));
    line(&mut r, redact_titles(&problems[problems.len().saturating_sub(80)..].join("\n"), keep_titles));
    let reports = crash::reports();
    line(&mut r, format!("---- crash / problem reports ({} saved; newest 3 below) ----", reports.len()));
    for p in reports.iter().take(3) {
        let mut text = std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if text.len() > 40_000 {
            let mut cut = 40_000;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str("\n…(cut)");
        }
        let text = redact_titles(&text, keep_titles);
        line(&mut r, format!("== {} ==\n{text}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    line(&mut r, "---- last 200 log lines ----".into());
    let lines: Vec<&str> = log.lines().collect();
    line(&mut r, redact_titles(&lines[lines.len().saturating_sub(200)..].join("\n"), keep_titles));
    hide_user(&r)
}

// ---------------------------------------------------------------- sending

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).collect::<String>() + "…" }
}

fn send(url: &str, kind: Kind, message: &str, name: &str, report: Option<&str>) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("hyprwin-feedback-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let from = if name.trim().is_empty() { "anonymous".to_string() } else { truncate(name.trim(), 100) };
    let payload = serde_json::json!({
        "username": "hyprwin feedback",
        "allowed_mentions": { "parse": [] },
        "embeds": [{
            "title": kind.title(),
            "description": truncate(if message.trim().is_empty() { "(no message)" } else { message.trim() }, 3900),
            "color": kind.color(),
            "fields": [
                { "name": "Version", "value": crate::update::VERSION, "inline": true },
                { "name": "Windows", "value": truncate(&windows_version(), 200), "inline": true },
                { "name": "From", "value": from, "inline": true },
                { "name": "Report", "value": if report.is_some() { "attached" } else { "not included" }, "inline": true },
            ],
        }],
    });
    let pj = dir.join("payload.json");
    std::fs::write(&pj, payload.to_string()).map_err(|e| e.to_string())?;
    let mut args: Vec<String> = vec!["-s".into(), "-m".into(), "30".into(), "-o".into(), "NUL".into(), "-w".into(), "%{http_code}".into()];
    args.push("-F".into());
    args.push(format!("payload_json=<{}", pj.display()));
    if let Some(rep) = report {
        let rf = dir.join("hyprwin-report.txt");
        std::fs::write(&rf, rep).map_err(|e| e.to_string())?;
        args.push("-F".into());
        args.push(format!("files[0]=@{};type=text/plain", rf.display()));
    }
    args.push(url.to_string());
    let out = std::process::Command::new("curl.exe").args(&args).creation_flags(NO_WINDOW).output().map_err(|_| "couldn't run curl".to_string());
    let _ = std::fs::remove_dir_all(&dir);
    let out = out?;
    let code = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match code.as_str() {
        "200" | "204" => Ok(()),
        "000" => Err("couldn't reach Discord (are you online?)".into()),
        "429" => Err("too many messages just now; try again in a minute".into()),
        c => Err(format!("Discord answered with error {c}")),
    }
}

fn desktop() -> PathBuf {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            if let Some(s) = s {
                return PathBuf::from(s);
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Desktop")).unwrap_or_else(std::env::temp_dir)
}

fn save_to_desktop(kind: Kind, message: &str, name: &str, report: &str) -> Result<PathBuf, String> {
    let stamp = crash::utc_stamp(SystemTime::now()).replace([':', ' '], "-");
    let path = desktop().join(format!("hyprwin-feedback-{stamp}.txt"));
    let text = format!("{}\nfrom: {}\n\n{}\n\n==================== diagnostic report ====================\n{report}", kind.title(), if name.trim().is_empty() { "anonymous" } else { name.trim() }, message.trim());
    std::fs::write(&path, text).map_err(|e| format!("couldn't save the report: {e}"))?;
    Ok(path)
}

fn reveal(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}

// ---------------------------------------------------------------- the tab

impl Feedback {
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Feedback");
        ui.label("Found a problem, or have an idea? Tell us here. It goes straight to the developer.");
        if crate::update::is_prerelease() {
            ui.label(RichText::new("You're on a test version, so every report really helps.").color(Color32::from_rgb(255, 190, 90)));
        }
        ui.add_space(8.0);

        if let Some(rx) = &self.sending {
            match rx.try_recv() {
                Ok(r) => {
                    self.sending = None;
                    self.result = Some(r.map(|()| "Sent. Thank you!".to_string()));
                    if matches!(self.result, Some(Ok(_))) {
                        self.last_sent = Some(Instant::now());
                        self.message.clear();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.sending = None;
                    self.result = Some(Err("sending stopped unexpectedly".into()));
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(Duration::from_millis(150)),
            }
        }

        egui::Grid::new("feedback-form").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
            ui.label("What's it about?");
            egui::ComboBox::from_id_salt("fb-kind").selected_text(self.kind.label()).width(260.0).show_ui(ui, |ui| {
                for k in [Kind::Bug, Kind::Idea, Kind::Other] {
                    ui.selectable_value(&mut self.kind, k, k.label());
                }
            });
            ui.end_row();
            ui.label("Your name (optional)");
            ui.add(egui::TextEdit::singleline(&mut self.name).hint_text("so we know who to thank").desired_width(260.0));
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.label(if self.kind == Kind::Bug { "What happened? What were you doing just before?" } else { "Your message" });
        ui.add(egui::TextEdit::multiline(&mut self.message).desired_rows(6).desired_width(f32::INFINITY).hint_text("For example: \"When I pressed Super + 2, my browser disappeared and didn't come back.\""));

        let reports = crash::reports().len();
        ui.add_space(4.0);
        ui.checkbox(&mut self.include_report, "Include a diagnostic report (recommended)");
        ui.indent("report-detail", |ui| {
            ui.label(
                RichText::new(format!(
                    "Version, Windows build, monitor setup, problems hyprwin noticed{} and its recent log. Window titles and your user name are hidden.",
                    if reports > 0 { format!(", and {reports} saved crash report(s)") } else { String::new() }
                ))
                .weak()
                .size(13.0),
            );
            ui.add_enabled(self.include_report, egui::Checkbox::new(&mut self.include_titles, "Also include window titles (can help with app-specific problems)"));
            if ui.add_enabled(self.include_report, egui::Button::new("Preview the report")).clicked() {
                self.preview = Some(build_report(self.include_titles));
            }
        });
        if let Some(p) = &mut self.preview {
            let mut open = true;
            egui::Window::new("Diagnostic report").open(&mut open).default_size([640.0, 460.0]).vscroll(true).show(ui.ctx(), |ui| {
                ui.add(egui::TextEdit::multiline(p).code_editor().desired_width(f32::INFINITY).interactive(false));
            });
            if !open {
                self.preview = None;
            }
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let sending = self.sending.is_some();
            let wait = self.last_sent.map(|t| SEND_GAP.saturating_sub(t.elapsed())).filter(|d| !d.is_zero());
            let empty = self.message.trim().is_empty() && !self.include_report;
            match webhook() {
                Some(url) => {
                    let label = if sending {
                        "Sending…".to_string()
                    } else if let Some(w) = wait {
                        format!("Send again in {} s", w.as_secs() + 1)
                    } else {
                        "Send".to_string()
                    };
                    let b = egui::Button::new(RichText::new(label).strong().size(15.0)).min_size(egui::Vec2::new(130.0, 32.0));
                    if ui.add_enabled(!sending && wait.is_none() && !empty, b).clicked() {
                        let (tx, rx) = channel();
                        let (kind, message, name, inc, titles) = (self.kind, self.message.clone(), self.name.clone(), self.include_report, self.include_titles);
                        let ctx = ui.ctx().clone();
                        std::thread::spawn(move || {
                            let report = inc.then(|| build_report(titles));
                            let _ = tx.send(send(url, kind, &message, &name, report.as_deref()));
                            ctx.request_repaint();
                        });
                        self.sending = Some(rx);
                        self.result = None;
                    }
                    if wait.is_some() {
                        ui.ctx().request_repaint_after(Duration::from_millis(500));
                    }
                }
                None => {
                    ui.label(RichText::new("This build can't send directly; save the report and send the file to the developer.").weak());
                }
            }
            if ui.button("Save to Desktop instead").clicked() {
                let report = build_report(self.include_titles);
                self.result = Some(save_to_desktop(self.kind, &self.message, &self.name, &report).map(|p| {
                    reveal(&p);
                    format!("Saved {} on your Desktop.", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                }));
            }
        });
        match &self.result {
            Some(Ok(m)) => {
                ui.label(RichText::new(m.as_str()).color(Color32::from_rgb(120, 220, 150)));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("Couldn't send: {e}. You can save it to the Desktop instead.")).color(Color32::from_rgb(240, 110, 110)));
            }
            None => {}
        }

        ui.add_space(14.0);
        ui.separator();
        ui.label(RichText::new("Crash and problem reports").strong());
        ui.label(
            RichText::new("hyprwin saves a report by itself if it crashes, stops unexpectedly, or the emergency key is used. They're included above when you send a diagnostic report.")
                .weak()
                .size(13.0),
        );
        ui.horizontal(|ui| {
            ui.label(if reports == 0 { "No reports saved. 👍".to_string() } else { format!("{reports} report(s) saved.") });
            if ui.button("Open the folder").clicked() {
                let d = crash::crashes_dir();
                let _ = std::fs::create_dir_all(&d);
                let _ = std::process::Command::new("explorer.exe").arg(&d).spawn();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_user_names_are_hidden() {
        let user = std::env::var("USERNAME").unwrap_or_default();
        let text = format!("INFO manage 0x1 'Inbox - Mail' class=x\nlog C:\\Users\\{user}\\AppData\\x\n");
        let r = redact(&text, false);
        assert!(!r.contains("Inbox"), "{r}");
        assert!(r.contains("manage 0x1 '…' class=x"), "{r}");
        if user.len() > 1 {
            assert!(!r.contains(&format!("\\{user}\\")), "{r}");
        }
        assert!(redact(&text, true).contains("Inbox - Mail"));
    }

    #[test]
    fn long_messages_are_cut_for_discord() {
        let s = "é".repeat(5000);
        assert_eq!(truncate(&s, 3900).chars().count(), 3900);
        assert_eq!(truncate("short", 3900), "short");
    }
}
