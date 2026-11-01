use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::rules::{Action, Kind};

#[derive(Debug, Clone)]
pub struct Resolved {
    pub target: String,
    pub ok: bool,
    pub why: String,
}

fn deny(target: impl Into<String>, why: &str) -> Resolved {
    Resolved {
        target: target.into(),
        ok: false,
        why: why.to_string(),
    }
}

pub fn which(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Security boundary: returns the resolved target plus whether removal is allowed.
///
/// Denies anything not provably safe to remove — relative paths, `..`, paths
/// outside home or tmp, the allowed roots themselves, shallow paths, and any
/// symlink at the target or at one of its ancestors. The caller must treat
/// `ok == false` as "never touch".
///
/// Symlinks are checked on the original path, component by component, before
/// canonicalising: canonicalising first would erase the evidence. The check is
/// not race-proof — another process could swap an ancestor between validation
/// and deletion.
pub fn resolve(action: &Action, home: &Path) -> Resolved {
    if action.kind == Kind::Cmd {
        let program = action.target.split_whitespace().next().unwrap_or("");
        return if which(program) {
            Resolved {
                target: action.target.clone(),
                ok: true,
                why: String::new(),
            }
        } else {
            deny(action.target.clone(), "not in PATH")
        };
    }

    let raw = Path::new(&action.target);
    if !raw.is_absolute() {
        return deny(action.target.clone(), "not an absolute path");
    }

    let mut prefix = PathBuf::from("/");
    for component in raw.components() {
        match component {
            Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => return deny(action.target.clone(), "contains .."),
            Component::Normal(name) => {
                prefix.push(name);
                match fs::symlink_metadata(&prefix) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return deny(
                            action.target.clone(),
                            &format!("symlink in path: {}", prefix.display()),
                        );
                    }
                    Ok(_) => {}
                    Err(_) => return deny(action.target.clone(), "does not exist"),
                }
            }
            _ => return deny(action.target.clone(), "unsupported path component"),
        }
    }

    let real = match fs::canonicalize(raw) {
        Ok(real) => real,
        Err(_) => return deny(action.target.clone(), "cannot resolve"),
    };

    let canonical_home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let roots: Vec<PathBuf> = [
        canonical_home,
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
    ]
    .into_iter()
    .map(|root| fs::canonicalize(&root).unwrap_or(root))
    .collect();

    if roots.iter().any(|root| root == &real) {
        return deny(real.to_string_lossy(), "is an allowed root");
    }
    if !roots.iter().any(|root| real.starts_with(root)) {
        return deny(real.to_string_lossy(), "outside allowed roots");
    }
    if real.components().count() < 3 {
        return deny(real.to_string_lossy(), "too shallow");
    }
    Resolved {
        target: real.to_string_lossy().into_owned(),
        ok: true,
        why: String::new(),
    }
}

pub fn du(path: &Path) -> u64 {
    Command::new("du")
        .arg("-sbx")
        .arg(path)
        .output()
        .ok()
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .and_then(|token| token.parse().ok())
        })
        .unwrap_or(0)
}

pub fn human(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "K", "M", "G", "T"] {
        if value < 1024.0 {
            return if unit == "B" {
                format!("{value:.0}B")
            } else {
                format!("{value:.1}{unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{value:.1}P")
}

pub fn nuke(path: &Path) -> bool {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return true,
    };
    if meta.file_type().is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
    fs::symlink_metadata(path).is_err()
}

pub fn parse_bytes(text: &str) -> u64 {
    let token = text
        .trim()
        .trim_matches('"')
        .split_whitespace()
        .next()
        .unwrap_or("");
    for (suffix, multiplier) in [
        ("GB", 1u64 << 30),
        ("MB", 1 << 20),
        ("KB", 1 << 10),
        ("B", 1),
    ] {
        if let Some(number) = token.strip_suffix(suffix) {
            return number
                .parse::<f64>()
                .map(|value| (value * multiplier as f64) as u64)
                .unwrap_or(0);
        }
    }
    0
}

pub fn docker_note() -> Option<(u64, usize)> {
    if !which("docker") {
        return None;
    }
    let out = Command::new("docker")
        .args(["system", "df", "--format", "{{json .}}"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let reclaimable = text
        .lines()
        .filter_map(|line| {
            line.split("\"Reclaimable\":\"")
                .nth(1)
                .map(|rest| parse_bytes(rest.split(',').next().unwrap_or("")))
        })
        .sum();
    let out = Command::new("docker")
        .args(["volume", "ls", "-f", "dangling=true", "-q"])
        .output()
        .ok()?;
    let dangling = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Some((reclaimable, dangling))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_reads_in_binary_units() {
        assert_eq!(human(0), "0B");
        assert_eq!(human(999), "999B");
        assert_eq!(human(1024), "1.0K");
        assert_eq!(human(1536), "1.5K");
        assert_eq!(human(8 * 1024 * 1024 * 1024), "8.0G");
    }

    #[test]
    fn parse_bytes_handles_docker_format() {
        assert_eq!(parse_bytes("\"8GB (99%)\""), 8 << 30);
        assert_eq!(parse_bytes("\"0B (0%)\""), 0);
        assert_eq!(parse_bytes("\"512MB\""), 512 << 20);
        assert_eq!(parse_bytes("\"4KB\""), 4 << 10);
        assert_eq!(parse_bytes(""), 0);
        assert_eq!(parse_bytes("garbage"), 0);
    }

    #[test]
    fn parse_bytes_is_monotonic() {
        assert!(parse_bytes("1MB") < parse_bytes("1GB"));
        assert!(parse_bytes("1KB") < parse_bytes("1MB"));
    }
}
