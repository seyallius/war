# war

Offline-first, airgap-ready dependency management, starting with Go.

[![License: MIT](https://img.shields.io/badge/license-MIT)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-stable)](https://rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.16%2B-00ADD8)](https://golang.org)
[![Crates.io](https://img.shields.io/badge/version-0.1.0--alpha-orange)](https://crates.io/crates/war)

---

The internet got cut. The proxy is blocked. The secure server has no connection.

Standard tools assume the network is always there. `go mod vendor` gives you source, but toolchains still try to reach
out, or they miss system-level dependencies.

`war` is designed for true **sneakernet / airgap workflows**. It allows you to build a portable, offline-ready cache of
all your dependencies on an online machine, transport it via USB/archive, and seamlessly inject it into an offline
environment using `file://` protocols.

This tool was born out of necessity. My country's internet is heavily restricted, and I wanted to keep developing at
home, not just at the office. So I built `war`.

---

## The Airgap Workflow

**1. On the Online Machine (Gather & Pack)**

```bash
# Initialize war in your project
war go init

# Fetch dependencies (adds to project AND stages them in a "shopping cart")
war go get github.com/gin-gonic/gin

# Pack everything into a portable archive for the USB drive
war go pack cache.zip            # Bundles ALL cached modules
war go pack cache.zip --staged   # Bundles only modules staged by 'war go get'
```

**2. On the Offline Machine (Unpack & Build)**

```bash
# Unpack the archive (Additively merges into ~/.war/cache)
war go unpack cache.zip

# Drop into offline mode (Configures GOPROXY="file://~/.war/cache/go")
eval $(war go offline)

# Build as usual — zero network, pure local cache
go mod tidy && go build ./...   # Or...
war go verify                   # Cleaner, right?

# (Optional) Hydrate native Go cache permanently
war go sync
```

---

## Architecture

`war` is a Cargo workspace. Each crate has one job:

- `war-cli` — thin binary, parses args via `clap`, dispatches to domain crates.
- `war-core` — shared types: `WarError`, `war.lock` config, shell detection.
- `war-go` — Go domain logic: `init`, `get`, `pack`, `unpack`, `sync`, offline shell orchestration.
- `war-tui` *(future)* — `ratatui` frontend, same domain logic underneath.
- `war-cargo` *(future)* — Rust's cargo offline support, identical architecture.

Adding a new language means adding a new sibling crate. Nothing else changes.

---

## Implementation Phases

| Phase | Focus                       | Deliverable                                                                              | Status    |
|-------|-----------------------------|------------------------------------------------------------------------------------------|-----------|
| **0** | Workspace bootstrap         | `Cargo.toml` workspace, crate stubs, `WarError`, CLI arg parsing                         | ✅ Done    |
| **1** | Basic Commands              | `war go init`, `war go get` (basic fetch), project scaffolding                           | ✅ Done    |
| **2** | The Airgap Pivot (Refactor) | Refactor `go offline` to use `eval` exports (`GOPROXY=file://...`), drop old vendor hack | ✅ Done    |
| **3** | Transport Layer             | Implement `war go pack` and `war go unpack` (Zip creation/extraction, additive unpack)   | ✅ Done    |
| **4** | The "Shopping Cart"         | Upgrade `war go get` to auto-stage modules; add `--staged` vs `--all` to `pack` command  | ✅ Done    |
| **5** | Cache Synchronization       | Implement `war go sync` to hydrate `$GOPATH/pkg/mod` from unpacked archives              | ✅ Done    |
| **6** | `verify` & Polish           | `war go verify`, offline status report, cross-platform CI, `--verbose` tracing           | 🚧 WIP    |
| **7** | `war-tui` & `war-cargo`     | Ratatui terminal UI and Rust ecosystem support                                           | 🔮 Future |

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
```

---

## Requirements

- Rust 1.75+
- Go 1.16+ (in `$PATH`)

---

## License

MIT

---

**Built with** ❤️ **for developers behind firewalls**  
*"Code should not require permission from a network"*
