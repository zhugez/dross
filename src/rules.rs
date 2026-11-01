use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Safe,
    Cost,
    Manual,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Safe, Tier::Cost, Tier::Manual];

    pub fn parse(text: &str) -> Option<Tier> {
        match text {
            "safe" => Some(Tier::Safe),
            "cost" => Some(Tier::Cost),
            "manual" => Some(Tier::Manual),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Safe => "safe",
            Tier::Cost => "cost",
            Tier::Manual => "manual",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    File,
    Cmd,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub tier: Tier,
    pub kind: Kind,
    pub target: String,
    pub label: &'static str,
}

impl Action {
    pub fn new(tier: Tier, kind: Kind, target: impl Into<String>, label: &'static str) -> Action {
        Action {
            tier,
            kind,
            target: target.into(),
            label,
        }
    }
}

// Docker volumes are reported, never pruned: they routinely hold live
// databases. Trash and journal are recoverable user data, so they belong to
// `manual` rather than `safe`.
pub fn defaults(home: &Path) -> Vec<Action> {
    let cache = home.join(".cache");
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    vec![
        Action::new(
            Tier::Safe,
            Kind::Dir,
            path(cache.join("libdnf5")),
            "dnf metadata cache",
        ),
        Action::new(
            Tier::Safe,
            Kind::Dir,
            path(cache.join("go-build")),
            "Go build cache",
        ),
        Action::new(
            Tier::Safe,
            Kind::Dir,
            path(cache.join("jstangle")),
            "jstangle cache",
        ),
        Action::new(
            Tier::Safe,
            Kind::Dir,
            path(cache.join("codeburn")),
            "codeburn cache",
        ),
        Action::new(
            Tier::Safe,
            Kind::Dir,
            path(home.join(".npm/_cacache")),
            "npm cache",
        ),
        Action::new(Tier::Safe, Kind::Cmd, "dnf clean all", "dnf package cache"),
        Action::new(
            Tier::Cost,
            Kind::Dir,
            path(cache.join("chromium")),
            "Chromium asset cache",
        ),
        Action::new(
            Tier::Cost,
            Kind::Dir,
            path(cache.join("electron")),
            "Electron download cache",
        ),
        Action::new(
            Tier::Cost,
            Kind::Dir,
            path(cache.join("ms-playwright")),
            "Playwright browsers",
        ),
        Action::new(
            Tier::Cost,
            Kind::Dir,
            path(cache.join("huggingface")),
            "HuggingFace models",
        ),
        Action::new(
            Tier::Manual,
            Kind::Dir,
            path(home.join(".local/share/Trash")),
            "Trash (recoverable files)",
        ),
        Action::new(
            Tier::Manual,
            Kind::Cmd,
            "journalctl --vacuum-size=200M",
            "journal logs -> 200M",
        ),
        Action::new(
            Tier::Manual,
            Kind::Cmd,
            "docker image prune -af",
            "docker images",
        ),
        Action::new(
            Tier::Manual,
            Kind::Cmd,
            "docker builder prune -af",
            "docker build cache",
        ),
    ]
}
