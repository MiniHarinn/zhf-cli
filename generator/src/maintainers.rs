use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::hydra::Build;

/// Max parallel `nix eval` processes. Keep low to avoid hammering the Nix daemon.
const PARALLEL_NIX_EVALS: usize = 8;

/// Resolves maintainer GitHub handles for all failed builds in parallel.
///
/// Returns a map of `attrpath → [github_handles]`.
pub async fn resolve_all(
    builds: &[Build],
    nixos_commit: &str,
    nixpkgs_commit: &str,
) -> HashMap<String, Vec<String>> {
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
            let maintainers = eval_maintainers(&nix_attr, &commit, is_nixos).await;
            Some((attrpath, maintainers))
        }));
    }

    let mut result = HashMap::new();
    for handle in handles {
        if let Ok(Some((attrpath, maintainers))) = handle.await {
            result.insert(attrpath, maintainers);
        }
    }
    result
}

/// Runs `nix eval --json` to get the maintainers list for a single attribute.
async fn eval_maintainers(nix_attr: &str, commit: &str, is_nixos: bool) -> Vec<String> {
    let (nix_file, attr) = if is_nixos {
        (
            "<nixpkgs>/nixos/release-combined.nix".to_string(),
            format!("{nix_attr}.meta.maintainers"),
        )
    } else {
        (
            "<nixpkgs>/pkgs/top-level/release.nix".to_string(),
            format!("{nix_attr}.meta.maintainers"),
        )
    };

    let nixpkgs_url = format!(
        "nixpkgs=https://github.com/NixOS/nixpkgs/archive/{commit}.tar.gz"
    );

    let output = Command::new("nix")
        .args(["eval", "--json", "-f", &nix_file, &attr])
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
            return vec![];
        }
        Err(e) => {
            log::warn!("Could not spawn nix for {nix_attr}: {e}");
            return vec![];
        }
    };

    parse_maintainers(&output.stdout)
}

/// Parses `[{"github": "alice", ...}, ...]` → `["alice", ...]`
fn parse_maintainers(json: &[u8]) -> Vec<String> {
    let Ok(val) = serde_json::from_slice::<serde_json::Value>(json) else {
        return vec![];
    };
    let Some(arr) = val.as_array() else {
        return vec![];
    };
    arr.iter()
        .filter_map(|m| m.get("github")?.as_str().map(str::to_string))
        .collect()
}
