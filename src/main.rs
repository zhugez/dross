use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

use dross::ops::{docker_note, du, human, nuke, resolve, Resolved};
use dross::rules::{defaults, Action, Kind, Tier};

// A closed pipe (`dross | head`) is normal Unix, not a crash: exit quietly.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    }};
}

struct Entry {
    size: u64,
    action: Action,
    resolved: Resolved,
}

fn tier_index(tier: Tier) -> usize {
    Tier::ALL.iter().position(|t| *t == tier).unwrap_or(0)
}

fn home_dir() -> PathBuf {
    let raw = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    fs::canonicalize(&raw).unwrap_or(raw)
}

fn state_dir(home: &Path) -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
}

fn log(path: &Path, message: &str) {
    let Some(parent) = path.parent() else { return };
    let _ = fs::create_dir_all(parent);
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "unix={unix} {message}");
}

fn scan(actions: Vec<Action>, home: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = actions
        .into_iter()
        .map(|action| {
            let resolved = resolve(&action, home);
            let size = match action.kind {
                Kind::Cmd => 0,
                _ => match fs::symlink_metadata(&action.target) {
                    Ok(_) => du(Path::new(&action.target)),
                    Err(_) => 0,
                },
            };
            Entry {
                size,
                action,
                resolved,
            }
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.size));
    entries
}

fn report(entries: &[Entry], home: &Path) {
    let home_text = home.to_string_lossy().into_owned();
    let mut totals = [0u64; 3];
    out!("{:>8}  {:<6}  {:<26}  TARGET", "SIZE", "TIER", "ITEM");
    out!("{}", "-".repeat(92));
    for entry in entries {
        totals[tier_index(entry.action.tier)] += entry.size;
        let shown = entry.resolved.target.replace(&home_text, "~");
        let size = if entry.size > 0 {
            human(entry.size)
        } else {
            "-".to_string()
        };
        let why = if entry.resolved.ok {
            String::new()
        } else {
            format!("   [{}]", entry.resolved.why)
        };
        out!(
            "{:>8}  {:<6}  {:<26}  {}{}",
            size,
            entry.action.tier.as_str(),
            entry.action.label,
            shown,
            why
        );
    }
    out!("{}", "-".repeat(92));
    let summary: Vec<String> = Tier::ALL
        .iter()
        .map(|tier| format!("{}={}", tier.as_str(), human(totals[tier_index(*tier)])))
        .collect();
    out!("{}", summary.join("  "));
    if let Some((reclaimable, dangling)) = docker_note() {
        out!(
            "docker: ~{} reclaimable across all object types, {dangling} dangling volume(s) — never pruned automatically",
            human(reclaimable)
        );
        out!("        inspect: docker volume ls -f dangling=true");
    }
    out!("\ndross plan TIER | dross clean TIER   (safe < cost < manual)");
}

fn plan(tier: Tier, actions: Vec<Action>, home: &Path) {
    let entries: Vec<Entry> = scan(actions, home)
        .into_iter()
        .filter(|entry| entry.action.tier == tier)
        .collect();
    out!(
        "TIER {} — {} target(s), nothing deleted:",
        tier.as_str(),
        entries.len()
    );
    for entry in &entries {
        let status = if entry.resolved.ok { "OK  " } else { "SKIP" };
        let size = if entry.action.kind == Kind::Cmd {
            String::new()
        } else {
            human(entry.size)
        };
        let suffix = if entry.resolved.ok {
            String::new()
        } else {
            format!("  [{}]", entry.resolved.why)
        };
        out!(
            "  {status} {size:>7}  {:<26}  {}{}",
            entry.action.label,
            entry.resolved.target,
            suffix
        );
    }
}

fn clean(tier: Tier, actions: Vec<Action>, home: &Path, log_path: &Path) -> i32 {
    let entries: Vec<Entry> = scan(actions, home)
        .into_iter()
        .filter(|entry| entry.action.tier == tier)
        .collect();
    if entries.is_empty() {
        eprintln!("nothing at tier '{}'", tier.as_str());
        return 1;
    }

    let mut removed: u64 = 0;
    let mut failed: i32 = 0;
    for entry in entries {
        let action = &entry.action;
        if !entry.resolved.ok {
            out!("  SKIP    {:<26} [{}]", action.label, entry.resolved.why);
            log(
                log_path,
                &format!(
                    "SKIP tier={} {} reason={}",
                    tier.as_str(),
                    entry.resolved.target,
                    entry.resolved.why
                ),
            );
            continue;
        }

        log(
            log_path,
            &format!(
                "BEGIN tier={} kind={:?} target={}",
                tier.as_str(),
                action.kind,
                entry.resolved.target
            ),
        );

        if action.kind == Kind::Cmd {
            let argv: Vec<&str> = entry.resolved.target.split_whitespace().collect();
            match Command::new(argv[0]).args(&argv[1..]).output() {
                Ok(out) if out.status.success() => {
                    out!("  ok      {:<26} (command)", action.label);
                    log(log_path, &format!("OK cmd {}", entry.resolved.target));
                }
                Ok(out) => {
                    failed += 1;
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let last = stderr.lines().last().unwrap_or("?").to_string();
                    out!(
                        "  FAIL    {:<26} rc={} {}",
                        action.label,
                        out.status.code().unwrap_or(-1),
                        last
                    );
                    log(
                        log_path,
                        &format!(
                            "FAIL cmd {} rc={:?} {}",
                            entry.resolved.target,
                            out.status.code(),
                            last
                        ),
                    );
                }
                Err(err) => {
                    failed += 1;
                    out!("  FAIL    {:<26} {err}", action.label);
                    log(
                        log_path,
                        &format!("FAIL cmd {} {err}", entry.resolved.target),
                    );
                }
            }
            continue;
        }

        let path = Path::new(&entry.resolved.target);
        let (before, got, left) = if action.kind == Kind::Dir {
            let before = du(path);
            if let Ok(read) = fs::read_dir(path) {
                for item in read.flatten() {
                    nuke(&item.path());
                }
            }
            let after = du(path);
            let left = fs::read_dir(path)
                .map(|mut r| r.next().is_some())
                .unwrap_or(false);
            (before, before.saturating_sub(after), left)
        } else {
            let gone = nuke(path);
            let got = if gone { entry.size } else { 0 };
            (entry.size, got, !gone)
        };

        removed += got;
        if left {
            failed += 1;
            out!(
                "  FAIL    {:<26} partially removed ({} of {})",
                action.label,
                human(got),
                human(before)
            );
            log(
                log_path,
                &format!(
                    "PARTIAL {} removed={} of {}",
                    entry.resolved.target, got, before
                ),
            );
            continue;
        }
        out!("  removed {:>7}  {}", human(got), action.label);
        log(
            log_path,
            &format!(
                "OK tier={} {} removed={} before={}",
                tier.as_str(),
                entry.resolved.target,
                got,
                before
            ),
        );
    }

    out!(
        "\nRemoved ~{} (bytes deleted, not necessarily disk reclaimed on COW filesystems)",
        human(removed)
    );
    if failed > 0 {
        out!("{failed} target(s) failed");
    }
    out!("log: {}", log_path.display());
    if tier != Tier::Safe {
        out!("WSL: ext4.vhdx does not shrink on its own. In PowerShell:");
        out!("     wsl --manage <distro> --set-sparse true");
    }
    failed
}

fn usage() -> ! {
    eprintln!("usage: dross | dross plan {{safe|cost|manual}} | dross clean {{safe|cost|manual}}");
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let home = home_dir();
    let actions = defaults(&home);
    let log_path = state_dir(&home).join("dross.log");

    let code = match (args.first().map(String::as_str), args.get(1)) {
        (None, _) => {
            report(&scan(actions, &home), &home);
            0
        }
        (Some("plan"), Some(tier)) => match Tier::parse(tier) {
            Some(tier) => {
                plan(tier, actions, &home);
                0
            }
            None => usage(),
        },
        (Some("clean"), Some(tier)) => match Tier::parse(tier) {
            Some(tier) => {
                if clean(tier, actions, &home, &log_path) > 0 {
                    1
                } else {
                    0
                }
            }
            None => usage(),
        },
        _ => usage(),
    };
    process::exit(code);
}
