<div align="center">

# dross

**Conservative disk cleaner for Fedora and WSL.**

Read-only by default · every deletion measured · nothing that cannot be rebuilt

[![ci](https://github.com/zhugez/dross/actions/workflows/ci.yml/badge.svg)](https://github.com/zhugez/dross/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](Cargo.toml)
[![dependencies](https://img.shields.io/badge/dependencies-none-brightgreen.svg)](Cargo.toml)

</div>

`dross` is the scum that floats to the top of molten metal — the part you skim
off and throw away. The tool follows the same rule: it removes only what it can
prove is worthless and regenerable, and it tells you what it is about to do
before it does it.

```
    SIZE  TIER    ITEM                        TARGET
--------------------------------------------------------------------------------------------
    8.2G  safe    npm cache                   ~/.npm/_cacache
  223.8M  safe    dnf metadata cache          ~/.cache/libdnf5
  218.9M  safe    Go build cache              ~/.cache/go-build
  118.9M  safe    codeburn cache              ~/.cache/codeburn
   98.4M  safe    jstangle cache              ~/.cache/jstangle
       -  safe    dnf package cache           dnf clean all
       -  cost    Chromium asset cache        ~/.cache/chromium
       -  cost    HuggingFace models          ~/.cache/huggingface
       -  manual  Trash (recoverable files)   ~/.local/share/Trash
       -  manual  journal logs -> 200M        journalctl --vacuum-size=200M
       -  manual  docker images               docker image prune -af
--------------------------------------------------------------------------------------------
safe=8.9G  cost=0B  manual=0B
docker: ~69.4G reclaimable across all object types, 8 dangling volume(s) — never pruned automatically
        inspect: docker volume ls -f dangling=true
```

## Install

```sh
cargo install --path .        # or: cargo build --release
```

No dependencies outside coreutils (`du`) and whatever command actions you have
installed.

## Usage

```sh
dross                  # read-only report, deletes nothing
dross plan safe        # print exactly what would be removed, deletes nothing
dross clean safe       # remove safe tier, verify each target, log everything
dross --help           # tiers, log location
```

Exit codes: `0` success, `1` at least one target failed or was partially
removed, `2` bad arguments.

## Tiers

| Tier | Meaning | Examples |
|---|---|---|
| `safe` | Pure cache, cheap to rebuild | npm + npx caches, dnf metadata, Go build cache |
| `cost` | Regenerable but expensive | uv/pip/cargo/pnpm caches, Puppeteer and Rod browsers |
| `manual` | Needs judgement | journal logs, Trash, docker prune, `dnf autoremove`, old kernels |

## What it will not do

- **No deletion without `clean`.** Report and `plan` are strictly read-only.
- **No docker volume pruning.** Volumes routinely hold live databases, so they
  are never removed. The report shows Docker-wide reclaimable space and, on a
  separate line, how many volumes are dangling — it is deliberately *not* a
  claim about how much space those volumes hold.
- **No guessing about paths.** Every target goes through a resolver that
  rejects: paths outside `$HOME`, `/tmp` and `/var/tmp`; root directories;
  paths shallower than three components; and any symlink at the target or at
  one of its ancestors.
- **No deleting installed software.** `~/.local/share/uv/tools` (uv-installed
  tools) and `~/.rustup/toolchains` are large and look cache-like, but removing
  them uninstalls working software. They are deliberately absent from the table.
- **No silent success.** A target that still exists after deletion counts as a
  failure and makes the process exit non-zero. Sizes are measured with `du`
  before and after, not assumed from the earlier scan.

Every action is appended to `$XDG_STATE_HOME/dross.log` (default
`~/.local/state/dross.log`) with its resolved path and outcome, so a surprise
can be traced afterwards.

The resolver is **not race-proof**: another process could swap a directory for a
symlink between validation and deletion. Closing that gap needs descriptor-
relative, no-follow traversal. Treat `dross` as a careful tool, not a
privileged one.

## WSL

Deleting files inside WSL does not shrink the virtual disk. After a cleanup, in
PowerShell:

```powershell
wsl --manage <distro> --set-sparse true
```

## Customising

There is no config file. The target list is a table in
[`src/rules.rs`](src/rules.rs); add a line, rebuild.

The tests cover the resolver and the deletion accounting, not whether a new
entry is a sensible thing to delete. Adding a path is a judgement call the
tool cannot make for you.

## Development

```sh
cargo test              # 11 integration + 3 unit tests
cargo clippy --all-targets -- -D warnings
```

Tests run against a temporary `$HOME` with a `PATH` that contains only `du`, so
they never execute `dnf`, `journalctl` or `docker` on the host.

## License

MIT
