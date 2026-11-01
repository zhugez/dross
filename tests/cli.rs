use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use dross::rules::{Action, Kind, Tier};
use dross::{resolve, Tier as T};

struct Sandbox {
    root: PathBuf,
}

fn find_du() -> PathBuf {
    for candidate in ["/usr/bin/du", "/bin/du"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return path;
        }
    }
    for dir in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = dir.join("du");
        if path.is_file() {
            return path;
        }
    }
    panic!("du not found; dross needs coreutils");
}

impl Sandbox {
    fn new() -> Sandbox {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = env::temp_dir().join(format!("dross-test-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create sandbox");
        Sandbox { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn write(&self, relative: &str, bytes: usize) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, vec![b'x'; bytes]).expect("write file");
        path
    }

    // PATH holds only `du`, so every command action resolves to "not in PATH"
    // and is skipped: tests never run dnf, journalctl or docker for real.
    fn bin_dir(&self) -> PathBuf {
        let dir = self.path("bin");
        fs::create_dir_all(&dir).expect("create bin");
        let link = dir.join("du");
        if !link.exists() {
            std::os::unix::fs::symlink(find_du(), &link).expect("symlink du");
        }
        dir
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dross"))
            .args(args)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.path("state"))
            .env("PATH", self.bin_dir())
            .output()
            .expect("run dross")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn report_and_plan_delete_nothing() {
    let sandbox = Sandbox::new();
    let blob = sandbox.write(".cache/go-build/blob", 4096);
    let model = sandbox.write(".cache/huggingface/model", 4096);

    let report = sandbox.run(&[]);
    assert!(report.status.success(), "report should succeed");
    assert!(blob.exists() && model.exists(), "report must not delete");

    let plan = sandbox.run(&["plan", "safe"]);
    assert!(plan.status.success(), "plan should succeed");
    assert!(blob.exists() && model.exists(), "plan must not delete");
    assert!(stdout_of(&plan).contains("nothing deleted"));
}

#[test]
fn clean_safe_empties_cheap_caches_and_keeps_expensive_ones() {
    let sandbox = Sandbox::new();
    let go_cache = sandbox.write(".cache/go-build/sub/deep", 4096);
    let npm_cache = sandbox.write(".npm/_cacache/blob", 4096);
    let models = sandbox.write(".cache/huggingface/model", 4096);

    let output = sandbox.run(&["clean", "safe"]);

    assert!(
        output.status.success(),
        "clean safe should succeed: {output:?}"
    );
    assert!(!go_cache.exists(), "nested file must be deleted");
    assert!(!npm_cache.exists(), "npm cache file must be deleted");
    assert!(models.exists(), "cost tier must be untouched by clean safe");
}

#[test]
fn clean_skips_commands_missing_from_path() {
    let sandbox = Sandbox::new();
    sandbox.write(".cache/go-build/blob", 1024);

    let output = sandbox.run(&["clean", "safe"]);
    let text = stdout_of(&output);

    assert!(text.contains("SKIP"), "dnf should be skipped, got: {text}");
    assert!(text.contains("not in PATH"));
}

#[test]
fn symlinked_target_is_denied() {
    let sandbox = Sandbox::new();
    let outside = sandbox.path("outside");
    fs::create_dir_all(&outside).expect("create outside");
    let keep = outside.join("keep");
    fs::write(&keep, b"keep").expect("write keep");
    fs::create_dir_all(sandbox.path(".cache")).expect("create cache");
    std::os::unix::fs::symlink(&outside, sandbox.path(".cache/go-build")).expect("symlink");

    let output = sandbox.run(&["clean", "safe"]);

    assert!(keep.exists(), "data behind a symlink must never be deleted");
    assert!(stdout_of(&output).contains("symlink"));
}

#[test]
fn resolve_denies_roots_relative_paths_and_dotdot() {
    let sandbox = Sandbox::new();
    let home = fs::canonicalize(&sandbox.root).expect("canonical home");
    let probe = |target: &str| resolve(&Action::new(Tier::Safe, Kind::Dir, target, "probe"), &home);

    assert!(!probe("/").ok, "/ must be denied");
    assert!(!probe("/tmp").ok, "/tmp must be denied");
    assert!(!probe("/var/tmp").ok, "/var/tmp must be denied");
    assert!(!probe("/etc").ok, "/etc must be denied");
    assert!(
        !probe(&home.to_string_lossy()).ok,
        "home itself must be denied"
    );
    assert!(!probe("relative/path").ok, "relative path must be denied");
    assert!(!probe("/tmp/../etc").ok, ".. must be denied");
}

#[test]
fn ancestor_symlink_is_denied_inside_and_outside_home() {
    let sandbox = Sandbox::new();
    let home = fs::canonicalize(&sandbox.root).expect("canonical home");
    let probe = |target: PathBuf| {
        resolve(
            &Action::new(
                Tier::Safe,
                Kind::Dir,
                target.to_string_lossy().to_string(),
                "probe",
            ),
            &home,
        )
    };

    let inside = sandbox.path("real-inside");
    fs::create_dir_all(inside.join("inner")).expect("mkdir inside");
    fs::write(inside.join("inner/keep"), b"keep").expect("write inside");
    fs::create_dir_all(sandbox.path(".cache")).expect("mkdir cache");
    std::os::unix::fs::symlink(&inside, sandbox.path(".cache/link-inside")).expect("symlink");

    let resolved = probe(sandbox.path(".cache/link-inside/inner"));
    assert!(
        !resolved.ok,
        "symlinked ancestor inside home must be denied"
    );
    assert!(resolved.why.contains("symlink"), "reason: {}", resolved.why);
    assert!(inside.join("inner/keep").exists(), "data must survive");

    let outside = sandbox.path("real-outside");
    fs::create_dir_all(outside.join("inner")).expect("mkdir outside");
    fs::write(outside.join("inner/keep"), b"keep").expect("write outside");
    std::os::unix::fs::symlink(&outside, sandbox.path(".cache/link-outside")).expect("symlink");

    let resolved = probe(sandbox.path(".cache/link-outside/inner"));
    assert!(
        !resolved.ok,
        "symlinked ancestor outside home must be denied"
    );
    assert!(resolved.why.contains("symlink"), "reason: {}", resolved.why);
    assert!(outside.join("inner/keep").exists(), "data must survive");
}

#[test]
fn partial_removal_exits_nonzero() {
    if fs::metadata("/proc/self").is_err() {
        return;
    }
    let sandbox = Sandbox::new();
    let locked = sandbox.path(".cache/go-build/locked");
    fs::create_dir_all(&locked).expect("create locked");
    fs::write(locked.join("f"), b"x").expect("write locked file");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).expect("chmod");

    let output = sandbox.run(&["clean", "safe"]);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("chmod back");
    if output.status.code() == Some(0) {
        // Running as root: permissions do not block removal, nothing to assert.
        return;
    }
    assert_eq!(output.status.code(), Some(1), "partial removal must exit 1");
    assert!(stdout_of(&output).contains("partially removed"));
}

#[test]
fn version_and_help_exit_zero() {
    let sandbox = Sandbox::new();
    let version = sandbox.run(&["--version"]);
    assert_eq!(version.status.code(), Some(0));
    assert!(stdout_of(&version).starts_with("dross "));

    let help = sandbox.run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout_of(&help).contains("usage:"));
}

#[test]
fn bad_arguments_exit_two() {
    let sandbox = Sandbox::new();
    assert_eq!(sandbox.run(&["clean", "everything"]).status.code(), Some(2));
    assert_eq!(sandbox.run(&["plan"]).status.code(), Some(2));
    assert_eq!(sandbox.run(&["--nonsense"]).status.code(), Some(2));
}

#[test]
fn tier_roundtrips() {
    for tier in T::ALL {
        assert_eq!(T::parse(tier.as_str()), Some(tier));
    }
    assert_eq!(T::parse("everything"), None);
}

#[test]
fn closed_pipe_exits_quietly() {
    let Some(head) = ["/usr/bin/head", "/bin/head"]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
    else {
        return;
    };
    let sandbox = Sandbox::new();
    sandbox.write(".cache/go-build/blob", 4096);
    let bin = sandbox.bin_dir();
    std::os::unix::fs::symlink(&head, bin.join("head")).expect("symlink head");

    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{} | head -1", env!("CARGO_BIN_EXE_dross")))
        .env("HOME", &sandbox.root)
        .env("XDG_STATE_HOME", sandbox.path("state"))
        .env("PATH", &bin)
        .output()
        .expect("run through sh");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "closed pipe must not panic: {stderr}"
    );
    assert_eq!(output.status.code(), Some(0), "closed pipe must exit 0");
}
