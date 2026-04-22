use std::collections::{HashMap, HashSet};
use tokio::process::Command;

use crate::hydra::Build;

#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

/// Bulk-resolve maintainers for every attrpath in `builds` using a single
/// `nix-env -qaP --json --meta` invocation against the pinned nixpkgs commit.
///
/// This is the same shape `flake-info` uses to feed search.nixos.org: one
/// process amortizes the nixpkgs import cost across the entire release set,
/// avoiding the per-batch re-import that the previous `nix eval --expr` loop
/// paid. Returns a map keyed by `Build::attrpath` (with the `nixos.` /
/// `nixpkgs.` prefix preserved) so callers can look up by the same key they
/// already store.
pub async fn resolve_all(
    builds: &[Build],
    commit: &str,
    is_nixos: bool,
) -> HashMap<String, MetaInfo> {
    // Deduplicate attrpaths and remember the corresponding nix-env lookup key.
    // Hydra stores `attrpath` with a "nixos."/"nixpkgs." prefix; nix-env's
    // attrPath is relative to the release.nix root, i.e. without that prefix.
    let mut wanted: HashMap<String, String> = HashMap::new(); // lookup_key -> build.attrpath
    for build in builds {
        let lookup = lookup_key(&build.nix_attr, is_nixos).to_string();
        wanted.entry(lookup).or_insert_with(|| build.attrpath.clone());
    }
    if wanted.is_empty() {
        return HashMap::new();
    }

    let nix_file = if is_nixos {
        "<nixpkgs/nixos/release.nix>"
    } else {
        "<nixpkgs/pkgs/top-level/release.nix>"
    };

    log::info!(
        "Bulk-fetching maintainer metadata via nix-env -qaP (is_nixos={is_nixos}, wanted={})",
        wanted.len()
    );

    let nixpkgs_url = format!("nixpkgs=https://github.com/NixOS/nixpkgs/archive/{commit}.tar.gz");

    let output = Command::new("nix-env")
        .args([
            "-qaP",
            "--json",
            "--meta",
            "--file",
            nix_file,
            "--arg",
            "config",
            "{ allowBroken = true; allowUnfree = true; allowInsecure = true; }",
        ])
        .env("NIX_PATH", &nixpkgs_url)
        .env("NIXPKGS_ALLOW_BROKEN", "1")
        .env("NIXPKGS_ALLOW_UNFREE", "1")
        .env("NIXPKGS_ALLOW_INSECURE", "1")
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            log::warn!(
                "nix-env -qaP failed (is_nixos={is_nixos}): {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return HashMap::new();
        }
        Err(e) => {
            log::warn!("Could not spawn nix-env: {e}");
            return HashMap::new();
        }
    };

    let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("nix-env --json parse error: {e}");
            return HashMap::new();
        }
    };

    // nix-env --json emits either an object (keyed by pkg-name with inner
    // attrPath) or, on newer versions, an array of entries. Handle both.
    let entries: Vec<(Option<String>, &serde_json::Value)> = match &parsed {
        serde_json::Value::Object(obj) => obj
            .iter()
            .map(|(k, v)| (Some(k.clone()), v))
            .collect(),
        serde_json::Value::Array(arr) => arr.iter().map(|v| (None, v)).collect(),
        _ => {
            log::warn!("nix-env --json returned unexpected top-level shape");
            return HashMap::new();
        }
    };

    let mut matched: HashSet<String> = HashSet::new();
    let mut result: HashMap<String, MetaInfo> = HashMap::new();

    for (outer_key, entry) in entries {
        let attrpath = entry
            .get("attrPath")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or(outer_key);
        let Some(attrpath) = attrpath else { continue };

        let Some(full_attrpath) = wanted.get(&attrpath) else {
            continue;
        };

        let maintainers = entry
            .get("meta")
            .and_then(|m| m.get("maintainers"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("github")?.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        matched.insert(attrpath);
        result.insert(full_attrpath.clone(), MetaInfo { maintainers });
    }

    log::info!(
        "Maintainers: matched {}/{} wanted attrs for is_nixos={is_nixos}",
        matched.len(),
        wanted.len()
    );

    result
}

/// Convert a Hydra `nix_attr` to the attrPath nix-env emits for the
/// corresponding release.nix root (no leading `nixos.`).
fn lookup_key<'a>(nix_attr: &'a str, is_nixos: bool) -> &'a str {
    if is_nixos {
        nix_attr.strip_prefix("nixos.").unwrap_or(nix_attr)
    } else {
        nix_attr
    }
}
