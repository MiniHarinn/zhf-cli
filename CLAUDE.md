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

## Commit Message Convention

Use conventional commits with short subject only (no description):
- `fix: <short subject>`
- `feat: <short subject>`
- `docs: <short subject>`
- `refactor: <short subject>`

Examples: `fix: populate maintainers in output JSON`, `docs: add CLAUDE.md`

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

**Reading generated files:**
When reading generated JSON files in `./output/data/`, use `head` or `tail` to conserve tokens, as these files can be very large (280k+ build entries).

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
  - `maintainers.rs`: Looks up maintainers in the pre-computed `packages.json.br` channel artifact from `channels.nixos.org`

### Data Pipeline

1. **Fetch Evals**: Get latest completed nixos/unstable and nixpkgs/unstable from `/jobset/{project}/{jobset}/latest-eval`
2. **Fetch Builds**: Parse eval HTML at `/eval/{id}?full=1` to extract failed jobs, platforms, statuses (HTML parser in `hydra.rs`)
3. **Resolve Maintainers**: Fetch `packages.json.br` for `nixos-unstable` and `nixpkgs-unstable` from `channels.nixos.org`, brotli-decompress, and look up `meta.maintainers[].github` per failing attrpath
4. **Output**: Write `output/data/{direct,indirect,index}.json`

### Key Implementation Notes

- **Hydra integration**: Uses HTML parsing instead of JSON API because the eval endpoint times out on large evals (280k+ builds). The HTML parser extracts job names, platforms, build IDs, and failure status.
- **Maintainers resolution**: Downloads the pre-computed `packages.json.br` channel artifacts (same dump `search.nixos.org` indexes) for `nixos-unstable` and `nixpkgs-unstable`, brotli-decompresses streamingly, and projects to `attrpath → [github handles]`. No `nix` subprocess at runtime. Gaps: `nixosTests.*` and aggregate Hydra jobs aren't packages, so they come back with empty maintainers — expected, not a regression.
- **Build deduplication**: Generator deduplicates by attrpath, preferring direct failures over indirect.
- **TCP keepalive**: Generator sets TCP keepalive to 30s since Hydra can take minutes to serialize large eval responses.

### CLI Filters

- **By attrset**: `--nixpkgs` or `--nixos`
- **By platform**: `--fails-on {all,linux,darwin,aarch64-linux,x86_64-linux,aarch64-darwin,x86_64-darwin,i686-linux}`
- **By maintainer**: `--maintainer <github-handle>` or `--no-maintainer`
- **Export**: `--export <path>` writes filtered results as CSV

## Common Patterns

- **Async/await**: Generator uses tokio for concurrent Hydra HTTP fetches and channel-artifact downloads
- **Error handling**: Uses `anyhow::Result<T>` throughout
- **HTTP**: reqwest with custom user-agent; no automatic retries (none needed for Hydra endpoints)
- **Logging**: Generator uses `env_logger` (no logging in CLI)

## Known Constraints

- No tests in the repository
- Generator requires network access to `hydra.nixos.org` and `channels.nixos.org`; `nix` is only used for the dev shell, not at runtime
- Large eval runs (280k+ builds) can take 30+ minutes; dominated by Hydra HTML fetches + dependency resolution, not maintainer lookup
