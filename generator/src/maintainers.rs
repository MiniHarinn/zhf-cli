use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::hydra::Build;

/// Max parallel `nix eval` processes. Keep low to avoid hammering the Nix daemon.
const PARALLEL_NIX_EVALS: usize = 8;

/// Package metadata resolved via `nix eval`.
#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

/// Resolves package metadata for all failed builds in parallel.
///
/// Returns a map of `attrpath → MetaInfo`.
pub async fn resolve_all(
    builds: &[Build],
    nixos_commit: &str,
    nixpkgs_commit: &str,
) -> HashMap<String, MetaInfo> {
    let sem = Arc::new(Semaphore::new(PARALLEL_NIX_EVALS));
    let mut handles = Vec::new();

    for build in builds {
        let nix_attr = build.nix_attr.clone();
        let attrpath = build.attrpath.clone();
        let is_nixos = build.is_nixos;
        let commit = if is_nixos {
            nixos_commit.to_string()
        } else {
            nixpkgs_commit.to_string()
        };
        let sem = sem.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let meta = eval_meta(&nix_attr, &commit, is_nixos).await;
            Some((attrpath, meta))
        }));
    }

    let mut result = HashMap::new();
    for handle in handles {
        if let Ok(Some((attrpath, meta))) = handle.await {
            result.insert(attrpath, meta);
        }
    }
    result
}

/// Runs `nix eval --json` to get package metadata for a single attribute.
async fn eval_meta(nix_attr: &str, commit: &str, is_nixos: bool) -> MetaInfo {
    let nix_file = if is_nixos {
        "nixpkgs/nixos/release-combined.nix"
    } else {
        "nixpkgs/pkgs/top-level/release.nix"
    };

    let expr = format!(
        "let pkg = (import <{nix_file}> {{}}).{nix_attr}; \
         in {{ maintainers = pkg.meta.maintainers or []; }}"
    );

    let nixpkgs_url = format!(
        "nixpkgs=https://github.com/NixOS/nixpkgs/archive/{commit}.tar.gz"
    );

    let output = Command::new("nix")
        .args(["eval", "--json", "--impure", "--expr", &expr])
        .env("NIX_PATH", &nixpkgs_url)
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            log::debug!(
                "nix eval failed for {nix_attr}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return MetaInfo::default();
        }
        Err(e) => {
            log::warn!("Could not spawn nix for {nix_attr}: {e}");
            return MetaInfo::default();
        }
    };

    parse_meta_info(&output.stdout)
}

/// Parses `{"maintainers": [...]}` into `MetaInfo`.
fn parse_meta_info(json: &[u8]) -> MetaInfo {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(json) else {
        return MetaInfo::default();
    };

    let maintainers = val
        .get("maintainers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("github")?.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    MetaInfo { maintainers }
}
