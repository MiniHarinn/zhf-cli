# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

**zhf-cli** is a Rust CLI tool for querying NixOS/nixpkgs build failures from Hydra CI. It consists of two main programs:

1. **Generator** (`generator/`): Async tool that pulls failure data from Hydra and resolves maintainers
2. **CLI** (`src/`): Interactive query tool that filters and displays failures

The generator produces JSON files that the CLI reads. Data is published from `output/data/` (direct.json, indirect.json, index.json).

## Development Setup

All commands should run via `nix develop`:

```bash
nix develop --command <command>
```

The flake provides Rust toolchain, openssl, and pkg-config. Never use bare `cargo`; always use `nix develop --command cargo`.

## Common Commands

**Build:**
```bash
nix develop --command cargo build --workspace
```

**Run CLI help:**
```bash
nix develop --command cargo run -- --help
```

**Run specific CLI command:**
```bash
nix develop --command cargo run -- stats
nix develop --command cargo run -- direct --nixpkgs --fails-on x86_64-linux
```

**Run generator (fetches from Hydra, may take 10+ minutes):**
```bash
nix develop --command cargo run -p zhf-generator
```

## Codebase Architecture

### Workspace Structure

- **`crates/types/`**: Shared data types (IndexJson, FailureItem, FailureCounts, EvalInfo)
- **`src/`**: Main CLI binary
  - `main.rs`: Command dispatcher and failure filtering logic
  - `cli.rs`: Clap command/arg definitions
  - `fetcher.rs`: HTTP client for reading output JSON files
  - `table.rs`: Display formatting and CSV export
- **`generator/src/`**: Data generation pipeline
  - `main.rs`: Orchestrates: fetch evals → fetch builds → resolve maintainers → write JSON
  - `hydra.rs`: Hydra API integration; parses eval HTML (JSON API times out on large evals)
  - `maintainers.rs`: Resolves maintainers for each attrpath via `nix eval --expr`

### Data Pipeline

1. **Fetch Evals**: Get latest completed nixos/unstable and nixpkgs/unstable from `/jobset/{project}/{jobset}/latest-eval`
2. **Fetch Builds**: Parse eval HTML at `/eval/{id}?full=1` to extract failed jobs, platforms, statuses (HTML parser in `hydra.rs`)
3. **Resolve Maintainers**: For each attrpath, run `nix eval --impure --expr '(import <nixpkgs/...> {}).{attr}.meta.maintainers'` with the nixpkgs commit pinned via NIX_PATH
4. **Output**: Write `output/data/{direct,indirect,index}.json`

### Key Implementation Notes

- **Hydra integration**: Uses HTML parsing instead of JSON API because the eval endpoint times out on large evals (280k+ builds). The HTML parser extracts job names, platforms, build IDs, and failure status.
- **Maintainers resolution**: Uses `nix eval --impure --expr` (not `-f`) to properly resolve `<nixpkgs>` paths; NIX_PATH is set to fetch from GitHub at a specific commit.
- **Build deduplication**: Generator deduplicates by attrpath, preferring direct failures over indirect.
- **Parallel evals**: Maintainer resolution runs with a semaphore limiting concurrency to 8 nix eval processes (avoid overwhelming Nix daemon).
- **TCP keepalive**: Generator sets TCP keepalive to 30s since Hydra can take minutes to serialize large eval responses.

### CLI Filters

- **By attrset**: `--nixpkgs` or `--nixos`
- **By platform**: `--fails-on {all,linux,darwin,aarch64-linux,x86_64-linux,aarch64-darwin,x86_64-darwin,i686-linux}`
- **By maintainer**: `--maintainer <github-handle>` or `--no-maintainer`
- **Export**: `--export <path>` writes filtered results as CSV

## Common Patterns

- **Async/await**: Generator uses tokio for concurrent nix eval calls
- **Error handling**: Uses `anyhow::Result<T>` throughout
- **HTTP**: reqwest with custom user-agent; no automatic retries (none needed for Hydra endpoints)
- **Logging**: Generator uses `env_logger` (no logging in CLI)

## Known Constraints

- No tests in the repository
- Generator requires working `nix` and network access to Hydra and GitHub
- Large eval runs (280k+ builds) can take 30+ minutes
- Maintainer resolution is the longest phase; semaphore limits to 8 parallel nix evals to avoid daemon contention
