use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::hydra::Build;

// Pre-computed channel dumps published by channels.nixos.org. Same artifact
// search.nixos.org indexes — produced by flake-info/nix-env --meta once per
// channel bump, so we avoid running nix locally at all.
//
// The -staging and -staging-next channels don't publish packages.json.br
// (verified: 404). For those jobsets we reuse the -unstable artifact and
// accept that maintainers can drift by a few days; maintainers.nix changes
// rarely enough that this is not a meaningful regression.
const NIXOS_UNSTABLE_URL: &str = "https://channels.nixos.org/nixos-unstable/packages.json.br";
const NIXPKGS_UNSTABLE_URL: &str = "https://channels.nixos.org/nixpkgs-unstable/packages.json.br";

#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

pub struct MetaLookup {
    by_attr: HashMap<String, Vec<String>>,
}

impl MetaLookup {
    /// Fetch the two -unstable channel artifacts concurrently, decompress
    /// brotli, parse JSON, and merge into one attrpath → github-handles map.
    pub async fn fetch(client: &reqwest::Client) -> Result<Self> {
        let (nixos_map, nixpkgs_map) = futures::try_join!(
            fetch_channel(client, "nixos-unstable", NIXOS_UNSTABLE_URL),
            fetch_channel(client, "nixpkgs-unstable", NIXPKGS_UNSTABLE_URL),
        )?;

        let mut by_attr = nixpkgs_map;
        // nixos-unstable covers jobset-specific attrs (module tests absent; but
        // any nixos. attr that is a real package lives here). Merge it on top
        // — on overlap both channels agree on maintainers, so order is
        // cosmetic.
        by_attr.extend(nixos_map);

        log::info!("Maintainers lookup built: {} attrs indexed", by_attr.len());
        Ok(Self { by_attr })
    }

    /// Look up maintainers for each build and key the result by
    /// `build.attrpath` (preserving the `nixos.`/`nixpkgs.` prefix that the
    /// rest of the pipeline uses). Empty `maintainers` is the expected miss
    /// case — nixosTests.* and aggregate Hydra jobs aren't packages and
    /// aren't in the artifact.
    pub fn resolve(&self, builds: &[Build]) -> HashMap<String, MetaInfo> {
        let mut out: HashMap<String, MetaInfo> = HashMap::new();
        let mut with_meta = 0usize;
        for b in builds {
            if out.contains_key(&b.attrpath) {
                continue;
            }
            // packages.json keys are raw attrpaths — no `nixos.` prefix and
            // no trailing platform. Hydra job names embed both.
            let platform_suffix = format!(".{}", b.platform);
            let without_platform = b
                .nix_attr
                .strip_suffix(&platform_suffix)
                .unwrap_or(&b.nix_attr);
            let lookup_key = without_platform
                .strip_prefix("nixos.")
                .unwrap_or(without_platform);
            let maintainers = self.by_attr.get(lookup_key).cloned().unwrap_or_default();
            if !maintainers.is_empty() {
                with_meta += 1;
            }
            out.insert(b.attrpath.clone(), MetaInfo { maintainers });
        }
        log::info!(
            "Maintainers resolved: {}/{} attrs have at least one maintainer",
            with_meta,
            out.len()
        );
        out
    }
}

async fn fetch_channel(
    client: &reqwest::Client,
    label: &str,
    url: &str,
) -> Result<HashMap<String, Vec<String>>> {
    log::info!("Fetching {label} channel artifact: {url}");
    let bytes = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("unexpected status for {url}"))?
        .bytes()
        .await
        .with_context(|| format!("reading body from {url}"))?;

    log::info!(
        "{label}: downloaded {:.1} MiB brotli, decompressing…",
        bytes.len() as f64 / (1024.0 * 1024.0)
    );

    // 4 KiB input buffer is the crate's documented default.
    let decoder = brotli::Decompressor::new(std::io::Cursor::new(bytes), 4096);
    // A BufReader keeps serde_json's small reads cheap — the decompressor
    // itself is unbuffered, so every serde_json read would otherwise walk
    // through the brotli state machine.
    let reader = std::io::BufReader::with_capacity(64 * 1024, decoder);

    let doc: PackagesDoc = serde_json::from_reader(reader)
        .with_context(|| format!("parsing {label} packages.json"))?;

    // Project to attrpath → [github]; drop every other field immediately so
    // peak RAM is dominated by this small map, not the full JSON tree.
    let map: HashMap<String, Vec<String>> = doc
        .packages
        .into_iter()
        .map(|(attr, entry)| {
            let handles: Vec<String> = entry
                .meta
                .maintainers
                .into_iter()
                .filter_map(|m| m.github)
                .collect();
            (attr, handles)
        })
        .collect();

    let non_empty = map.values().filter(|v| !v.is_empty()).count();
    log::info!(
        "{label}: {} attrs in lookup ({} with github handles)",
        map.len(),
        non_empty
    );
    Ok(map)
}

#[derive(Deserialize)]
struct PackagesDoc {
    #[serde(default)]
    packages: HashMap<String, PackageEntry>,
}

#[derive(Deserialize, Default)]
struct PackageEntry {
    #[serde(default)]
    meta: PackageMeta,
}

#[derive(Deserialize, Default)]
struct PackageMeta {
    #[serde(default)]
    maintainers: Vec<Maintainer>,
}

#[derive(Deserialize)]
struct Maintainer {
    #[serde(default)]
    github: Option<String>,
}
