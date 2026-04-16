<p align="center">
  <img src="assets/icon-full.svg" alt="ZHF-CLI icon" width="512" />
</p>

<div align="center">

# ZHF-CLI

A CLI tool for querying [NixOS](https://nixos.org/) / [nixpkgs](https://github.com/NixOS/nixpkgs) build failures from [Hydra CI](https://hydra.nixos.org/), built to help maintainers during the [ **Zero Hydra Failures** ] campaign. Also useful outside of ZHF for checking what's currently failing on Hydra.

<p>
  <a href="https://hydra.nixos.org/"><img src="https://img.shields.io/badge/data-Hydra_CI-e05151?style=for-the-badge" alt="Hydra CI" height="28" /></a>
</p>

</div>

## Usage

Run directly via Nix flake:

```bash
nix run github:MiniHarinn/zhf-cli -- --help
```

Or from a local checkout:

```bash
nix run -- --help
```

### Examples

```bash
# Show build stats across channels
nix run github:MiniHarinn/zhf-cli -- stats

# List direct failures on x86_64-linux
nix run github:MiniHarinn/zhf-cli -- direct --fails-on x86_64-linux

# Show packages causing the most cascade failures (sorted by blocked count)
nix run github:MiniHarinn/zhf-cli -- problematic

# Only show cascade causes blocking at least 10 packages
nix run github:MiniHarinn/zhf-cli -- problematic --min-blocked 10

# Find failures for a specific maintainer
nix run github:MiniHarinn/zhf-cli -- direct --maintainer someuser

# Find packages with no maintainer
nix run github:MiniHarinn/zhf-cli -- direct --no-maintainer

# Export results as CSV
nix run github:MiniHarinn/zhf-cli -- direct --export failures.csv

# Query a specific channel
nix run github:MiniHarinn/zhf-cli -- direct --channel nixos:unstable
```

## How it works

Querying Hydra directly for all failed builds is slow and resource-intensive, and those resources are better spent on actual builds. To avoid that, a **generator** runs on GitHub Actions every 6 hours, pulling failure data from Hydra once and publishing it to GitHub Pages. The **CLI** then fetches that pre-built data instead of hitting Hydra directly.

```mermaid
flowchart LR
    subgraph Generator["Generator (every 6h)"]
        A[Fetch latest evals\nfrom Hydra] --> B[Parse eval HTML\nfor failed builds]
        B --> C[Resolve maintainers\nvia nix eval]
        C --> D[Resolve cascade chains\nfor problematic causes]
        D --> E[Diff vs. previous\nstate.json]
        E --> F[Write JSON + Atom feeds\ndirect / indirect / problematic\nper-channel / per-maintainer]
    end

    F --> G[GitHub Pages]
    G --> H[CLI fetches JSON]
    G --> I[RSS/Atom reader]
    H --> J[Filter & display\nresults]
```

The generator parses Hydra's eval HTML instead of the JSON API, which times out on large evals. Maintainer info comes from `nix eval` against the pinned nixpkgs commit. For each indirect failure, the generator follows Hydra's "propagated from" link to find the root cause, which lets `problematic` rank direct failures by how many packages they block.

Atom feeds are diff-based. Each run fetches the previously-published `state.json` from GitHub Pages, keeps `first_seen` for packages still failing, and only emits entries for ones that weren't failing last run. CI stays stateless and subscribers only get notified on new breakages.

### Subscribing to Atom feeds

Paste these URLs into any RSS/Atom reader (NetNewsWire, Feedly, Thunderbird, Inoreader):

- **Per-channel**: `https://zhf.harinn.dev/feed/{channel}.xml`. Channels: `nixos_unstable`, `nixos_staging`, `nixpkgs_unstable`, `nixpkgs_staging_next`.
- **Per-maintainer**: `https://zhf.harinn.dev/feed/maintainer/{github-handle}.xml`. Only your packages. Exists once you have at least one failing package.

Each feed keeps up to 200 entries, sorted by first-seen time. Direct and indirect (cascade) failures are both included.

### Note to Hydra maintainers

The generator only runs every 6 hours via GitHub Actions and makes a small number of requests per run. If this is still causing issues for Hydra (User-Agent: `zhf-generator/0.1`), please reach out at `matrix:@harinn:matrix.org`.

## Contributing

PRs are welcome! Please use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

---

<p align="center">Made with ❤️ by <a href="https://github.com/MiniHarinn">@MiniHarinn</a></p>
