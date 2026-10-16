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

pub fn parse_file(path: &Path) -> Parsed {
    let mut p = Parsed::default();
    let mut vars: Vec<(String, String)> = Vec::new();
    match std::fs::read_to_string(path) {
        Ok(text) => parse_into(&text, path, &mut vars, &mut p, 0),
        Err(e) => p.warnings.push(format!("{}: cannot read: {e}", path.display())),
    }
    p
}

pub fn parse_str(text: &str, name: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut vars = Vec::new();
    parse_into(text, Path::new(name), &mut vars, &mut p, 0);
    p
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

fn parse_into(text: &str, file: &Path, vars: &mut Vec<(String, String)>, out: &mut Parsed, depth: usize) {
    if depth > 16 {
        out.warnings.push(format!("{}: source nesting too deep", file.display()));
        return;
    }
    out.files.push(file.to_path_buf());
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
        let v = expand_vars(v.trim(), vars);
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
                match std::fs::read_to_string(&path) {
                    Ok(t) => parse_into(&t, &path, vars, out, depth + 1),
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
}
