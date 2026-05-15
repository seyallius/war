# war

Offline-first, airgap-ready dependency management for Go (and soon Rust).

[![License: MIT](https://img.shields.io/badge/license-MIT)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-stable)](https://rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.16%2B-00ADD8)](https://golang.org)
[![Crates.io](https://img.shields.io/badge/version-0.1.0--alpha-orange)](https://crates.io/crates/war)

---

The internet got cut. The proxy is blocked. The secure server has no connection.

Standard tools assume the network is always there. `go mod vendor` gives you source, but toolchains
still try to reach out, or they miss system-level dependencies.

`war` is designed for true **sneakernet / airgap workflows**. It lets you build a portable, offline-ready
cache of all your dependencies on an online machine, transport it via USB/archive, and inject it into an
offline environment using the standard `file://` protocol — no patches, no wrappers, no eval tricks
required after `war go sync`.

> Born out of necessity: my country's internet is heavily restricted, and I wanted to keep developing
> at home, not just at the office. So I built `war`.

---

## Quick Start

```bash
# Build the binary
cargo build --release

# On the ONLINE machine → gather and pack
war go get github.com/gin-gonic/gin      # fetch + auto-stage
war go pack cache.zip --staged           # bundle only staged modules

# Copy cache.zip to USB / sneakernet / scp...

# On the OFFLINE machine → unpack, verify, build
war go unpack cache.zip
war go sync                              # hydrate native Go cache permanently
war go verify                            # confirm everything is ready
go build ./...                           # works natively, zero eval, zero network
```

---

## The Two Offline Modes

`war` gives you two ways to build offline. Pick the one that fits your workflow:

### Mode A — Session eval (quick, reversible)

```bash
war go unpack cache.zip
eval $(war go offline)        # sets GOPROXY=file://~/.war/cache/go in current shell
go mod tidy && go build ./... # uses war cache via GOPROXY
eval $(war go online)         # restore normal env when done
```

Pros: reversible, no changes to the system Go cache.
Cons: must `eval` in every new shell session.

### Mode B — Native sync (permanent, recommended)

```bash
war go unpack cache.zip
war go sync                   # copies ~/.war/cache/go → $GOMODCACHE permanently
go build ./...                # works in any shell, no eval required
```

Pros: works in every terminal, IDE, CI script, and tool that calls Go — no wrapper needed.

Cons: modifies `~/go/pkg/mod/cache/download`. Collisions are reported, never silently overwritten.

---

## Full Workflow Reference

### On the Online Machine

```bash
# 1. Initialise a new Go project (optional — works on existing projects too)
war go init my-project
cd my-project

# 2. Fetch modules (adds to go.mod AND stages them for packing)
war go get github.com/gin-gonic/gin
war go get golang.org/x/text@v0.3.7

# 3. Review what's staged
war go stage list

# 4. Pack
war go pack cache.zip            # pack everything in ~/.war/cache/go
war go pack cache.zip --staged   # pack only staged modules (smaller archive)
```

### On the Offline Machine

```bash
# 1. Unpack the archive into the war cache
war go unpack cache.zip
war go unpack cache.zip --dry-run  # preview what would be extracted

# 2. Choose your offline mode (A or B, see above)

# Mode A: session env
eval $(war go offline)

# Mode B: native sync (recommended)
war go sync                        # default: $GOMODCACHE / ~/go/pkg/mod/cache/download
war go sync --dest /custom/path    # override destination

# 3. Verify everything is ready
war go verify

# 4. Build with standard tooling
go mod tidy && go build ./...
go test ./...
```

---

## Command Reference

| Command                                          | Description                                                   |
|--------------------------------------------------|---------------------------------------------------------------|
| `war go init [name]`                             | Scaffold a minimal Go project with `go.mod` and `main.go`     |
| `war go get <module>`                            | Fetch a module, auto-import it, and stage it                  |
| `war go pack <output.zip> [--staged]`            | Archive the Go module cache for transfer                      |
| `war go unpack <archive> [--dry-run] [--staged]` | Extract an archive into the war cache                         |
| `war go stage list`                              | List all staged modules                                       |
| `war go stage add <module> <version>`            | Manually stage a module                                       |
| `war go stage remove <module> <version>`         | Remove a module from the staged list                          |
| `war go stage clear`                             | Clear all staged modules                                      |
| `war go offline [--global]`                      | Print shell exports to enable offline mode                    |
| `war go online [--global]`                       | Print shell exports to restore online mode                    |
| `war go sync [--cache dir] [--dest dir]`         | Hydrate native `$GOMODCACHE` from war cache                   |
| `war go verify`                                  | Audit offline readiness: GOPROXY, cache contents, module list |

---

## Verify Output

`war go verify` runs three independent checks and prints a structured report:

```
(◕‿◕✿) Running offline verification suite…
  ✔ [GOPROXY]         GOPROXY=file:///home/you/.war/cache/go ✔  (path exists)
  ✔ [cache contents]  War cache at /home/you/.war/cache/go ✔  (47 module version(s) present)
  ✔ [module list]     `go list -m all -mod=readonly` ✔  (12 module(s) resolved offline)
(≧◡≦) All checks passed — `go build` should work offline.
```

When something is wrong, the report tells you exactly what to run next:

```
  ✘ [GOPROXY] GOPROXY points to '/home/you/.war/cache/go' which does not exist on disk.
      → Run `war go unpack <archive>` to populate /home/you/.war/cache/go, then retry.
```

Exit codes: `0` = all checks passed (errors=0), `1` = at least one Error-severity finding.
Warnings (e.g. GOPROXY not set after `war go sync`) do not raise the exit code.

---

## Sync Safety

`war go sync` is designed to be safe to run on any machine, even one where the native
Go module cache already has content:

- **Idempotent**: files already present with matching SHA-256 hash are skipped.
- **Never overwrites modified files**: if a file exists in the native cache but has a
  different hash from the war cache, a collision warning is printed and the file is left
  untouched. You see exactly which files collide and their respective hashes.
- **Atomic writes**: each file is written to a `.war.tmp` sidecar and `rename`d into place.
  An interrupted sync leaves no partial files.

---

## Corrupted Archive Recovery

`war go unpack` handles corrupt archives gracefully:

```bash
# Deliberately corrupt a zip
truncate -s 50% cache.zip

# war reports a clear error and cleans up
war go unpack cache.zip
# ✘ Corrupted zip archive at cache.zip: Cannot read zip central directory: …
#     → Re-create the archive with `war go pack` or re-download it.
#     → To verify: `unzip -t cache.zip` should return exit 0.

echo $?  # → 1
```

- Structural corruption (truncated file, bad magic) → immediate `CorruptedArchive` error, no partial state.
- Per-entry corruption (bad CRC on one file) → that entry is counted as `failed`, extraction continues.
- In both cases `stats.failed > 0` propagates exit code `1`.

---

## Architecture

`war` is a Cargo workspace. Each crate has one job:

```
war/
├── war-cli/      # Thin binary: arg parsing (clap), dispatch, exit codes
├── war-core/     # Shared: WarError, war.lock config, shell detection
├── war-go/       # Go domain logic: init, get, pack, unpack, sync, verify, offline
├── war-tui/      # (future) ratatui terminal UI, same domain logic underneath
└── war-cargo/    # (future) Rust/cargo offline support, identical architecture
```

Adding a new language means adding one new sibling crate. Nothing else changes.

---

## Implementation Phases

| Phase | Focus                                                                       | Status    |
|-------|-----------------------------------------------------------------------------|-----------|
| **0** | Workspace bootstrap: `Cargo.toml`, crate stubs, `WarError`, CLI arg parsing | ✅ Done    |
| **1** | Basic commands: `war go init`, `war go get`, project scaffolding            | ✅ Done    |
| **2** | Airgap pivot: `eval $(war go offline)` with `GOPROXY=file://…`              | ✅ Done    |
| **3** | Transport layer: `war go pack` / `war go unpack` (zip, atomic, idempotent)  | ✅ Done    |
| **4** | Shopping cart: auto-stage on `get`; `--staged` flag on `pack`/`unpack`      | ✅ Done    |
| **5** | Cache sync: `war go sync` hydrates `$GOMODCACHE` permanently                | ✅ Done    |
| **6** | Verify & polish: rich `war go verify`, `CorruptedArchive`, tracing spans    | ✅ Done    |
| **7** | `war-tui` & `war-cargo`: Ratatui UI and Rust ecosystem support              | 🔮 Future |

---

## Testing Your Workflow

### Run the test suite

```bash
# All tests (unit + integration)
cargo test

# Only war-go tests (fastest feedback loop)
cargo test -p war-go

# A specific test by name
cargo test test_full_airgap_sync_to_native_cache

# Show tracing output during tests (useful for debugging)
RUST_LOG=debug cargo test -- --nocapture
```

### Real-world validation

Run these steps in order to confirm the full airgap workflow end-to-end:

```bash
# ── 1. Build ────────────────────────────────────────────────────────────
cargo build --release
export PATH="$PWD/target/release:$PATH"

# ── 2. Fetch modules on your online machine ─────────────────────────────
mkdir /tmp/war-demo && cd /tmp/war-demo
war go init demo && cd demo
war go get github.com/gin-gonic/gin

# ── 3. Pack ─────────────────────────────────────────────────────────────
war go pack /tmp/cache.zip --staged

# ── 4. Simulate the offline machine (new shell, no network env) ──────────
# (In practice, transfer cache.zip to the offline machine via USB / scp.)
mkdir /tmp/war-offline && cd /tmp/war-offline
war go init offline-demo && cd offline-demo

# ── 5. Unpack ───────────────────────────────────────────────────────────
war go unpack /tmp/cache.zip
war go unpack /tmp/cache.zip --dry-run  # preview first

# ── 6. Sync → native cache ───────────────────────────────────────────────
war go sync
# Expected: "X copied, 0 collisions, 0 failed"

# ── 7. Verify ────────────────────────────────────────────────────────────
war go verify
# Expected: all three checks ✔ and exit 0

# ── 8. Build natively — no eval, no GOPROXY override, no network ─────────
unset GOPROXY GONOSUMDB GOSUMDB GOFLAGS
go mod tidy && go build ./...
echo "Exit: $?"  # Must be 0
```

### Corrupted archive test

```bash
# Copy a real archive, then corrupt it
cp /tmp/cache.zip /tmp/bad.zip
truncate -s 50% /tmp/bad.zip

# war must report a clear error and exit 1
war go unpack /tmp/bad.zip
echo "Exit: $?"  # Must be 1

# Confirm no partial state was left behind
ls ~/.war/cache/go  # Should be unchanged from before
```

### Collision test

```bash
# Sync once → native cache is populated
war go sync

# Manually edit a file in the native cache to simulate user modification
echo "user modified" >> ~/go/pkg/mod/cache/download/github.com/\!gin-gonic/gin/@v/v1.9.1.mod

# Re-run sync → collision must be reported, file must be untouched
war go sync
# Expected: "⚠ 1 collision(s) detected — existing files were NOT overwritten"
```

### Verbose / structured logging

```bash
# Show all tracing spans (timing of pack/unpack/sync)
RUST_LOG=war_go=debug war go sync

# Show only info and above
RUST_LOG=info war go verify
```

---

## Installation

```bash
cargo install war-cli
```

Or build from source:

```bash
git clone https://github.com/seyallius/war
cd war
cargo build --release
./target/release/war --help
```

---

## Requirements

- **Rust** 1.75+
- **Go** 1.16+ (only required for `war go verify`'s `go list` check and `war go get`)

---

## License

MIT

---

**Built with** ❤️ **for developers behind firewalls**
*"Code should not require permission from a network."*