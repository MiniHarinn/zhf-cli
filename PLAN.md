# Self-Hosted Data Pipeline Plan

## Context

Replace zh.fail HTML scraping with self-hosted infrastructure:

- A **Rust generator** runs on GitHub Actions, queries Hydra's JSON API, evaluates nixpkgs for maintainers, and publishes split JSON files to **GitHub Pages**.
- The **CLI** fetches only the file it needs per command from GitHub Pages (`ZHF_DATA_URL` env var overrides the base URL for local testing).

---

## Architecture

```
[GitHub Actions — every 6 hours]
  → zhf-generator (Rust)
      → Hydra JSON API: nixos/unstable  → filter to nixos.* jobs only
      → Hydra JSON API: nixpkgs/unstable → all jobs (prepend nixpkgs. for display)
      → tokio::process::Command nix eval (maintainers, parallel, semaphore=8)
  → output/data/{index,direct,indirect}.json
  → GitHub Pages (actions/deploy-pages, no separate branch needed)
                          ↓
           [zhf CLI fetches only the file needed per command]
```

### Why two evals?

| Eval | Jobset | What it covers |
|---|---|---|
| `nixos/unstable` | nixos/release-combined.nix | NixOS tests, modules |
| `nixpkgs/unstable` | pkgs/top-level/release.nix | All nixpkgs packages |

Only `nixos.*` jobs are kept from the nixos eval; everything else comes from nixpkgs.

---

## JSON Schema

### `data/index.json` — `zhf stats`
```json
{
  "generated_at": "2026-04-14 12:00:00 (UTC)",
  "nixos_eval":   { "id": 1824463, "time": "2026-04-14 10:00:00 (UTC)" },
  "nixpkgs_eval": { "id": 1824458, "time": "2026-04-14 09:00:00 (UTC)" },
  "counts": {
    "aarch64_darwin": 42, "aarch64_linux": 137,
    "x86_64_darwin": 55,  "x86_64_linux": 201,
    "i686_linux": 5,      "total": 440
  }
}
```

### `data/direct.json` — `zhf direct` / `zhf all`
### `data/indirect.json` — `zhf indirect` / `zhf all`
```json
[
  {
    "attrpath": "nixpkgs.foo.x86_64-linux",
    "platform": "x86_64-linux",
    "maintainers": ["alice", "bob"],
    "hydra_url": "https://hydra.nixos.org/build/234567890"
  }
]
```

---

## File Structure

```
zhf/
├── .github/workflows/
│   └── generate-data.yml     ← GitHub Actions pipeline
├── crates/types/             ← shared JSON types (zhf-types crate)
│   ├── Cargo.toml
│   └── src/lib.rs            ← IndexJson, EvalInfo, FailureCounts, FailureItem
├── generator/                ← data generator binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs           ← orchestrator
│       ├── hydra.rs          ← Hydra JSON API client
│       └── maintainers.rs    ← parallel nix eval runner
├── src/                      ← CLI
│   ├── main.rs
│   ├── cli.rs
│   ├── fetcher.rs            ← replaces scraper.rs
│   └── table.rs
└── Cargo.toml                ← workspace root
```

---

## Hydra API Endpoints Used

| Endpoint | Purpose |
|---|---|
| `GET /jobset/{project}/{jobset}/latest-eval` | Get latest finished eval ID (follows 302) |
| `GET /eval/{id}` | Get eval details + nixpkgs commit |
| `GET /eval/{id}/builds` | Get all builds (returns flat JSON array) |

All requests use `Accept: application/json`. No HTML scraping.

Build status codes:
- `1`, `6`, `7`, `10`, `11` → direct failure
- `2` → indirect (dependency failed)
- Everything else → skip

---

## Maintainer Resolution

`nix eval --json -f <file> <attr>.meta.maintainers` via `tokio::process::Command`.

- `nixos.*` jobs → `<nixpkgs>/nixos/release-combined.nix`, nixos eval's commit
- `nixpkgs.*` jobs → `<nixpkgs>/pkgs/top-level/release.nix`, nixpkgs eval's commit

`NIX_PATH=nixpkgs=https://github.com/NixOS/nixpkgs/archive/{commit}.tar.gz` — no explicit git clone needed; `magic-nix-cache-action` caches the store path between runs.

Semaphore limits concurrent `nix eval` processes to 8.

---

## GitHub Pages Setup

1. Repo Settings → Pages → Source: **GitHub Actions** (not a branch)
2. Trigger `workflow_dispatch` to run the first time
3. Data URLs: `https://{user}.github.io/zhf/data/{index,direct,indirect}.json`

---

## Local Testing

```bash
# Build and run generator (requires Nix in PATH)
cargo build -p zhf-generator
./target/debug/zhf-generator
python -m http.server 8080 --directory output

# Test CLI against local data
ZHF_DATA_URL=http://localhost:8080 cargo run -- stats
ZHF_DATA_URL=http://localhost:8080 cargo run -- direct
ZHF_DATA_URL=http://localhost:8080 cargo run -- all --maintainer alice
```
