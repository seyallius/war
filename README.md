# war

Offline-first dependency management, starting with Go.

[![License: MIT](https://img.shields.io/badge/license-MIT)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-stable)](https://rust-lang.org)
[![Go](https://img.shields.io/badge/Go-1.16%2B-00ADD8)](https://golang.org)
[![Crates.io](https://img.shields.io/badge/version-0.1.0--alpha-orange)](https://crates.io/crates/war)

---

The internet got cut. The proxy is blocked. The flight has no WiFi.

`go mod vendor` gives you source, but `go build` still reaches out to `proxy.golang.org` — and it fails.
`war` fixes that. It takes your `vendor/` directory and reconstructs a complete Go module cache
(`$GOPATH/pkg/mod`) so you can build with zero network calls.

This tool was born out of necessity. My country's internet is heavily restricted, and I wanted
to keep developing at home, not just at the office. So I built `war`.

---

## How it works

```bash
# Initialize war in your Go project
war go init

# Fetch dependencies (while you still have internet)
war go get github.com/gin-gonic/gin

# Reconstruct the module cache from vendor/
war go offline

# Drop into offline mode
eval $(war go env)

# Build as usual — no network, no proxy, no problem
go build ./...

# Come back online when you're ready
war go online
```

---

## Architecture

`war` is a Cargo workspace. Each crate has one job:

- `war-cli` — thin binary, parses args via `clap`, dispatches to domain crates
- `war-core` — shared types: `WarError`, `war.lock` config, shell detection
- `war-go` — all Go-specific logic: init, get, vendor parsing, cache reconstruction, offline/online
- `war-tui` *(future)* — `ratatui` frontend, same domain logic underneath
- `war-rust` *(future)* — Cargo offline support, same architecture

Adding a new language means adding a new sibling crate. Nothing else changes.

---

## Implementation Phases

| Phase | Focus                          | Deliverable                                                                             |
|-------|--------------------------------|-----------------------------------------------------------------------------------------|
| **0** | Workspace bootstrap            | `Cargo.toml` workspace, all crate stubs, `WarError`, `war.lock` TOML read/write + tests |
| **1** | `war go init` + `war go get`   | Async process spawning, dummy project scaffolding, blank import injection, `go fmt`     |
| **2** | `vendor/modules.txt` parser    | Pure function: parse → `Vec<ModuleEntry>`, with property tests                          |
| **3** | Cache reconstruction core      | `.info`/`.mod`/`.zip` generation, `rayon` parallelism, atomic writes via `tempfile`     |
| **4** | `war go offline` orchestration | Env var management, interactive error handler (`dialoguer`), `war.lock` update          |
| **5** | `war go online` + `verify`     | Env restore, `go list`/`go build -x` checks, offline status report                      |
| **6** | Polish & docs                  | `--verbose` tracing, man pages, `CONTRIBUTING.md`, cross-platform CI                    |
| **7** | `war-tui` *(future)*           | Ratatui frontend consuming existing APIs                                                |

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
