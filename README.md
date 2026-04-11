# ⚔️ war

**Offline-first dependency management for Go**

[![Crates.io](https://img.shields.io/badge/version-0.1.0--alpha-orange)](https://crates.io/crates/war)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-stable)](https://rust-lang.org)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange)](https://www.rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.16%2B-00ADD8)](https://golang.org)
[![Tests](https://github.com/seyallius/war/actions/workflows/ci.yml/badge.svg)](https://github.com/seyallius/war/actions)
[![Documentation](https://img.shields.io/badge/docs-war.dev-blue)](https://war.dev)

## 🎯 The Problem

You're behind a firewall. Your country's internet is cut. Your flight has no WiFi.
But you have a `vendor/` directory and a mission to ship code.

`go mod vendor` gives you source, but `go build` still reaches out to `proxy.golang.org`.
Your IDE screams. Your CI fails. Your sanity erodes.

## 🔧 The Solution

**`war`** extracts your `vendor/` directory and reconstructs a complete Go module cache
(`$GOPATH/pkg/mod`) that works offline. No network. No proxy. No excuses.

```bash
# One-time setup in your project
war go init
war go get github.com/gorilla/mux

# Create offline cache from existing vendor/
war go offline

# Switch environment to offline mode (sets GOPROXY=off)
source <(war go env)  # Or eval (war go shell-init)

# Build normally - zero network calls
go build ./...

# Go back online when internet returns
war go online
```

## ✨ Features

- **🔒 Zero Network** - `GOPROXY=off`, `GOSUMDB=off` during offline builds
- **⚡ Parallel Cache Reconstruction** - Uses `rayon` to create `.zip` files for all modules concurrently
- **🔄 Interactive Recovery** - When a module fails, choose: Skip, Retry, Abort, or Debug
- **📦 Cross-Project** - Cache once, use across all Go projects on your machine
- **🎯 Preserves Integrity** - Stores original `go.sum` hashes for post-hoc verification
- **🖥️ TUI Coming Soon** - Terminal UI for managing multiple cache snapshots

## 🚀 Quick Start

### Installation

```bash
# From source
cargo install war-cli

# Or via pre-built binary (coming soon)
curl -fsSL https://github.com/seyallius/war/releases/latest/download/war-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv war /usr/local/bin
```

### Basic Workflow

```bash
# 1. Initialize war in your Go project
cd myproject
war go init

# 2. Add dependencies you need offline (or just run offline to use existing vendor/)
war go get github.com/gin-gonic/gin

# 3. Create offline cache from vendor/
war go offline

# 4. Enter offline mode
eval $(war go env)

# 5. Build as usual
go build ./...

# 6. Exit offline mode when done
war go online
```

## 📚 Commands

| Command            | Description                             |
|--------------------|-----------------------------------------|
| `war go init`      | Create dummy Go project in `~/.war/go/` |
| `war go get <pkg>` | Add dependency to offline cache plan    |
| `war go offline`   | Reconstruct module cache from `vendor/` |
| `war go online`    | Restore original Go environment         |
| `war go status`    | Show offline/online state               |
| `war go verify`    | Check cache integrity against `go.sum`  |
| `war go env`       | Output shell commands for offline mode  |

## 🏗️ Architecture

`war` is built as a Cargo workspace with clear separation of concerns:

```
war-cli (thin binary)
    ↓
war-go (Go-specific logic)
    ↓
war-core (shared: config, errors, shell detection)
```

- **`war-cli`**: Argument parsing with `clap`, dispatches to subcommands
- **`war-core`**: Shared types, `war.lock` (TOML), error handling, cross-platform paths
- **`war-go`**: Go module parser, cache reconstruction, `go` command orchestration
- **`war-tui`** (future): Ratatui-based terminal interface
- **`war-rust`** (future): Cargo offline support

## Workspace Structure

```bash
war/
├── Cargo.toml # [workspace] definition
├── war-cli/ # Thin binary crate
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
├── war-core/ # Shared domain logic
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── config.rs # war.lock (TOML), ~/.war/ dir management
│       ├── error.rs # WarError enum (thiserror)
│       ├── shell.rs # Shell detection, env var set/unset/persist
│       └── types.rs # Shared types (ModuleInfo, SyncResult, etc.)
├── war-go/ # All Go-specific knowledge
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── init.rs # war go init — scaffold dummy Go project
│       ├── get.rs # war go get — go get + blank import + tidy + vendor
│       ├── offline.rs # war go offline — env vars + cache reconstruction
│       ├── online.rs # war go online — restore env
│       ├── verify.rs # war go verify — go list / go build -x checks
│       ├── vendor.rs # Parse vendor/modules.txt, extract module metadata
│       └── cache.rs # Reconstruct ~/go/pkg/mod (.info, .mod, .zip per module)
├── war-rust/ # Future — All Rust-specific knowledge
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs
└── war-tui/ # Future — ratatui frontend (empty for now)
    ├── Cargo.toml
    └── src/
        └── lib.rs
```

## 🗓️ Revised Implementation Phases (With Testing)

| Phase | Focus                          | Deliverable                                                                             |
|-------|--------------------------------|-----------------------------------------------------------------------------------------|
| **0** | Workspace bootstrap            | `Cargo.toml` workspace, all crate stubs, `WarError`, `war.lock` TOML read/write + tests |
| **1** | `war go init` + `war go get`   | Async process spawning, dummy project scaffolding, blank import injection, `go fmt`     |
| **2** | `vendor/modules.txt` parser    | Pure function: parse → `Vec<ModuleEntry>`, with property tests                          |
| **3** | Cache reconstruction core      | `.info`/`.mod`/`.zip` generation, `rayon` parallelism, atomic writes via `tempfile`     |
| **4** | `war go offline` orchestration | Env var management, interactive error handler (`dialoguer`), `war.lock` update          |
| **5** | `war go online` + `verify`     | Env restore, `go list`/`go build -x` checks, offline status report                      |
| **6** | Polish & docs                  | `--verbose` tracing, man pages, `CONTRIBUTING.md`, cross-platform CI                    |
| **7** | `war-tui` (future)             | Ratatui frontend consuming existing APIs                                                |

## 🔄 How It Works

1. **Scan `vendor/modules.txt`** - Parse Go's vendor manifest
2. **Reconstruct module versions** - For each module, generate:
    - `.info` file (version metadata with timestamp)
    - `.mod` file (module requirements)
    - `.zip` file (actual source code)
3. **Place in `$GOPATH/pkg/mod/cache/download/`** - Where Go expects offline cache
4. **Set environment** - `GOPROXY=off`, `GOSUMDB=off`, `GOFLAGS=-mod=mod`
5. **Build offline** - Go uses reconstructed cache exclusively

## 🧪 Development

```bash
# Clone and build
git clone https://github.com/seyallius/war
cd war
cargo build --release

# Run tests across all crates
cargo test --workspace

# Run with verbose logging
RUST_LOG=debug war go offline
```

### Project Structure

```bash
war/
├── Cargo.toml              # Workspace definition
├── war-core/               # Shared: errors, config, lockfile
├── war-go/                 # Go module logic
├── war-cli/                # Binary entrypoint
├── war-tui/                # (future) Ratatui UI
└── war-test-utils/         # (planned) Shared test fixtures
```

## ⚠️ Limitations

- **Go only for now** - Rust support planned (Cargo offline cache)
- **Requires existing `vendor/`** - Run `go mod vendor` first
- **No checksum verification offline** - Warning logged; run `war go verify` when online
- **Windows untested** - Should work (thanks `dirs` crate), but CI coming

## 🤝 Contributing

PRs welcome! Areas needing help:

- Windows CI pipeline
- `war-tui` implementation (Ratatui)
- Property tests for `modules.txt` parser
- Performance benchmarks

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and conventions.

## 🛡️ License

MIT

---

**Built with** ❤️ **for developers behind firewalls**  
*"Code should not require permission from a network"*
