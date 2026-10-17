//! hyprlang → flat list of `(key, value)` entries.
//!
//! Handles `# comments` (`##` is a literal `#`), `$variables`, `source = path`,
//! nested `section { … }` blocks, `section:key = value`, and keyword blocks
//! such as `windowrule { … }`.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Lower-cased, `:`-joined path, e.g. `general:gaps_in`, `bind`, `bindel`.
    pub key: String,
    pub value: String,
    /// Key/value pairs for keyword blocks (`windowrule { … }`).
    pub block: Vec<(String, String)>,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
    /// Every file that was read (for hot reload).
    pub files: Vec<PathBuf>,
}

/// Keyword sections that produce one block entry instead of nested keys.
const BLOCK_KEYWORDS: &[&str] = &["windowrule", "layerrule", "workspace"];

/// Longest value a `$variable` may expand to: `$b = $a$a$a…` chains grow
/// tenfold per line and would exhaust memory within a dozen lines.
const MAX_VALUE: usize = 64 * 1024;
/// Most files one config may pull in through `source` (a file sourcing
/// itself twice would otherwise be read 2^16 times).
const MAX_FILES: usize = 256;

pub fn parse_file(path: &Path) -> Parsed {
    let mut p = Parsed::default();
    let mut vars: Vec<(String, String)> = Vec::new();
    match read_text(path, &mut p.warnings) {
        Ok(text) => parse_into(&text, path, &mut vars, &mut p, &mut Vec::new()),
        Err(e) => p.warnings.push(format!("{}: cannot read: {e}", path.display())),
    }
    p
}

/// Reads a config file whatever Notepad saved it as: UTF-8 (with or without
/// a BOM), UTF-16 ("Unicode"), or legacy "ANSI" bytes. Before, one non-UTF-8
/// byte (an accented comment) made the whole file unreadable, and hyprwin ran
/// with no config at all.
pub fn read_text(path: &Path, warnings: &mut Vec<String>) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(decode_text(&bytes, &path.display().to_string(), warnings))
}

fn decode_text(bytes: &[u8], name: &str, warnings: &mut Vec<String>) -> String {
    let utf16 = |be: bool| -> String {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => decode_text(rest, name, warnings),
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => match std::str::from_utf8(bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                warnings.push(format!("{name}: not UTF-8; read as Windows-1252 (save it as UTF-8 to be sure)"));
                // Windows-1252 differs from Latin-1 only in 0x80..0x9F; those are
                // punctuation that never matters to the config syntax.
                bytes.iter().map(|&b| b as char).collect()
            }
        },
    }
}

pub fn parse_str(text: &str, name: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut vars = Vec::new();
    parse_into(text, Path::new(name), &mut vars, &mut p, &mut Vec::new());
    p
}

/// Makes `value` safe to write as one hyprlang value: `#` would start a
/// comment (written `##`), and a line break would start a new line.
pub fn escape_value(value: &str) -> String {
    value.replace(['\r', '\n'], " ").replace('#', "##")
}

fn strip_comment(line: &str) -> String {
    // A line that starts with '#' is a comment even if it is all '#'s (banners).
    if line.trim_start().starts_with('#') {
        return String::new();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            if chars.peek() == Some(&'#') {
                chars.next();
                out.push('#');
                continue;
            }
            break;
        }
        out.push(c);
    }
    out
}

fn expand_vars(s: &str, vars: &[(String, String)]) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut out = s.to_string();
    // Longest names first so `$mainModShift` beats `$mainMod`.
    let mut sorted: Vec<&(String, String)> = vars.iter().collect();
    sorted.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    for (k, v) in sorted {
        out = out.replace(&format!("${k}"), v);
    }
    out
}

pub fn expand_home(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(s)
}

/// `chain` is the stack of files currently being read (for cycle detection).
fn parse_into(text: &str, file: &Path, vars: &mut Vec<(String, String)>, out: &mut Parsed, chain: &mut Vec<PathBuf>) {
    if chain.len() > 16 {
        out.warnings.push(format!("{}: source nesting too deep", file.display()));
        return;
    }
    out.files.push(file.to_path_buf());
    chain.push(file.to_path_buf());
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let fname = file.display().to_string();
    let mut sections: Vec<String> = Vec::new();
    let mut block: Option<(String, Vec<(String, String)>, usize)> = None;

    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "}" {
            if let Some((kw, kv, l)) = block.take() {
                out.entries.push(Entry { key: kw, value: String::new(), block: kv, file: fname.clone(), line: l });
            } else if sections.pop().is_none() {
                out.warnings.push(format!("{fname}:{lineno}: unmatched '}}'"));
            }
            continue;
        }
        if let Some(head) = line.strip_suffix('{') {
            let name = head.trim().to_lowercase();
            // `device[name] {` → section `device`
            let name = name.split('[').next().unwrap_or("").trim().to_string();
            if sections.is_empty() && BLOCK_KEYWORDS.contains(&name.as_str()) {
                block = Some((name, Vec::new(), lineno));
            } else {
                sections.push(name);
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            out.warnings.push(format!("{fname}:{lineno}: not a 'key = value' line: {line}"));
            continue;
        };
        let k = k.trim();
        let mut v = expand_vars(v.trim(), vars);
        if v.len() > MAX_VALUE {
            let mut cut = MAX_VALUE;
            while !v.is_char_boundary(cut) {
                cut -= 1;
            }
            v.truncate(cut);
            out.warnings.push(format!("{fname}:{lineno}: value longer than {MAX_VALUE} bytes after expanding variables; truncated"));
        }
        if let Some(name) = k.strip_prefix('$') {
            vars.retain(|(n, _)| n != name);
            vars.push((name.to_string(), v));
            continue;
        }
        if let Some((_, kv, _)) = block.as_mut() {
            kv.push((k.to_lowercase(), v));
            continue;
        }
        let mut key = k.to_lowercase();
        if !sections.is_empty() {
            key = format!("{}:{}", sections.join(":"), key);
        }
        // `bezier`/`animation` are keywords even inside `animations { }`.
        for kw in ["bezier", "animation"] {
            if key == format!("animations:{kw}") {
                key = kw.to_string();
            }
        }
        if key == "source" {
            for path in glob_paths(&expand_home(&v), file) {
                if chain.contains(&path) {
                    out.warnings.push(format!("{fname}:{lineno}: source {}: already being read (a source loop); skipped", path.display()));
                    continue;
                }
                if out.files.len() >= MAX_FILES {
                    out.warnings.push(format!("{fname}:{lineno}: source {}: more than {MAX_FILES} files sourced; skipped", path.display()));
                    continue;
                }
                match read_text(&path, &mut out.warnings) {
                    Ok(t) => parse_into(&t, &path, vars, out, chain),
                    Err(e) => out.warnings.push(format!("{fname}:{lineno}: source {}: {e}", path.display())),
                }
            }
            continue;
        }
        out.entries.push(Entry { key, value: v, block: Vec::new(), file: fname.clone(), line: lineno });
    }
    if !sections.is_empty() || block.is_some() {
        out.warnings.push(format!("{fname}: unclosed '{{' at end of file"));
    }
    chain.pop();
}

/// Resolves `source` paths: relative to the including file, `*` wildcard in the
/// final component.
fn glob_paths(p: &Path, including: &Path) -> Vec<PathBuf> {
    let p = if p.is_relative() { including.parent().map(|d| d.join(p)).unwrap_or_else(|| p.to_path_buf()) } else { p.to_path_buf() };
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !name.contains('*') {
        return vec![p];
    }
    let (pre, post) = name.split_once('*').unwrap();
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|q| q.file_name().map(|n| n.to_string_lossy().to_string()).is_some_and(|n| n.starts_with(pre) && n.ends_with(post)))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_vars_and_comments() {
        let p = parse_str(
            "$mod = SUPER # main\n$modShift = SUPER_SHIFT\ngeneral {\n  gaps_in = 5\n  col.active_border = rgba(33ccffee) ## not a comment\n}\nbind = $mod, Q, exec, wt\nbind = $modShift, Q, exit,\ndecoration:rounding = 10\n",
            "t",
        );
        let kv: Vec<(&str, &str)> = p.entries.iter().map(|e| (e.key.as_str(), e.value.as_str())).collect();
        assert_eq!(kv[0], ("general:gaps_in", "5"));
        assert_eq!(kv[1], ("general:col.active_border", "rgba(33ccffee) # not a comment"));
        assert_eq!(kv[2], ("bind", "SUPER, Q, exec, wt"));
        assert_eq!(kv[3], ("bind", "SUPER_SHIFT, Q, exit,"));
        assert_eq!(kv[4], ("decoration:rounding", "10"));
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }

    #[test]
    fn nested_sections_and_animation_keywords() {
        let p = parse_str("decoration {\n blur {\n  enabled = true\n }\n}\nanimations {\n bezier = b, 0,1,0,1\n animation = windows, 1, 7, b\n}\n", "t");
        let keys: Vec<&str> = p.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["decoration:blur:enabled", "bezier", "animation"]);
    }

    #[test]
    fn windowrule_block() {
        let p = parse_str("windowrule {\n  name = calc\n  match:class = calculator\n  float = on\n}\n", "t");
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].key, "windowrule");
        assert_eq!(p.entries[0].block.len(), 3);
    }

    #[test]
    fn unbalanced_braces_warn_not_fail() {
        let p = parse_str("general {\n gaps_in = 1\n", "t");
        assert_eq!(p.entries.len(), 1);
        assert!(!p.warnings.is_empty());
    }

    #[test]
    fn variable_bombs_are_capped() {
        let mut text = String::from("$a = xxxxxxxxxx\n");
        let names = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        for w in names.windows(2) {
            text += &format!("${} = {}\n", w[1], format!("${}", w[0]).repeat(10));
        }
        text += "general:gaps_in = $l\n";
        let t = std::time::Instant::now();
        let p = parse_str(&text, "bomb.conf");
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "took {:?}", t.elapsed());
        assert!(p.entries.iter().all(|e| e.value.len() <= MAX_VALUE));
        assert!(p.warnings.iter().any(|w| w.contains("truncated")));
    }

    #[test]
    fn source_loops_terminate_quickly() {
        let dir = std::env::temp_dir().join(format!("hyprwin-source-loop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.conf");
        let b = dir.join("b.conf");
        std::fs::write(&a, "source = a.conf\nsource = a.conf\nsource = b.conf\ngeneral:gaps_in = 1\n").unwrap();
        std::fs::write(&b, "source = a.conf\nsource = b.conf\nsource = *.conf\ngeneral:gaps_out = 2\n").unwrap();
        let t = std::time::Instant::now();
        let p = parse_file(&a);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "took {:?}", t.elapsed());
        assert!(p.files.len() <= MAX_FILES);
        assert!(p.warnings.iter().any(|w| w.contains("source loop")));
        assert!(p.entries.iter().any(|e| e.key == "general:gaps_out"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_notepad_encoding_loads() {
        let body = "# caf\u{e9}\ngeneral {\n    gaps_in = 7\n}\n";
        let utf16le: Vec<u8> = [0xFF, 0xFE].into_iter().chain(body.encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        let utf16be: Vec<u8> = [0xFE, 0xFF].into_iter().chain(body.encode_utf16().flat_map(|u| u.to_be_bytes())).collect();
        let bom8: Vec<u8> = [0xEF, 0xBB, 0xBF].into_iter().chain(body.bytes()).collect();
        let ansi: Vec<u8> = body.chars().map(|c| c as u32 as u8).collect();
        for (name, bytes) in [("utf16le", utf16le), ("utf16be", utf16be), ("utf8bom", bom8), ("ansi", ansi), ("utf8", body.as_bytes().to_vec())] {
            let mut w = Vec::new();
            let text = decode_text(&bytes, name, &mut w);
            let p = parse_str(&text, name);
            assert!(p.entries.iter().any(|e| e.key == "general:gaps_in" && e.value == "7"), "{name}: {:?} {:?}", p.entries, p.warnings);
            assert_eq!(w.is_empty(), name != "ansi", "{name}: {w:?}");
        }
    }
}
