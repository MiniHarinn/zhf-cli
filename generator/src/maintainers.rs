use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::hydra::Build;

/// Max parallel `nix eval` processes. Each process now evaluates a batch of attrs.
const PARALLEL_NIX_EVALS: usize = 8;
/// Number of attrpaths to resolve in a single `nix eval` invocation.
const BATCH_SIZE: usize = 50;

/// Package metadata resolved via `nix eval`.
#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

/// Resolves package metadata for all failed builds in parallel.
///
/// Deduplicates by attrpath (maintainers don't vary by platform), then batches
/// multiple attrpaths into a single `nix eval` call to amortize nixpkgs import cost.
///
/// Returns a map of `attrpath → MetaInfo`.
pub async fn resolve_all(
    builds: &[Build],
    nixos_commit: &str,
    nixpkgs_commit: &str,
) -> HashMap<String, MetaInfo> {
    // Deduplicate: one nix eval per unique attrpath (maintainers are per-package, not per-platform)
    let mut unique: HashMap<&str, (&str, &str, bool)> = HashMap::new();
    for build in builds {
        unique.entry(build.attrpath.as_str()).or_insert_with(|| {
            let commit = if build.is_nixos { nixos_commit } else { nixpkgs_commit };
            (build.nix_attr.as_str(), commit, build.is_nixos)
        });
    }

    // Group by (commit, is_nixos) so each batch shares a single nixpkgs import
    let mut groups: HashMap<(&str, bool), Vec<(&str, &str)>> = HashMap::new();
    for (attrpath, (nix_attr, commit, is_nixos)) in &unique {
        groups
            .entry((commit, *is_nixos))
            .or_default()
            .push((attrpath, nix_attr));
    }

    let total = unique.len();
    let completed = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(PARALLEL_NIX_EVALS));
    let mut handles = Vec::new();

    for ((commit, is_nixos), attrs) in groups {
        for chunk in attrs.chunks(BATCH_SIZE) {
            let chunk: Vec<(String, String)> = chunk
                .iter()
                .map(|(ap, na)| (ap.to_string(), na.to_string()))
                .collect();
            let commit = commit.to_string();
            let sem = sem.clone();
            let completed = completed.clone();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.ok()?;
                let result = eval_meta_batch(&chunk, &commit, is_nixos).await;
                let done = completed.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                log::info!("Maintainers: {done}/{total} attrs resolved");
                Some(result)
            }));
        }
    }

    let mut result = HashMap::new();
    for handle in handles {
        if let Ok(Some(batch_result)) = handle.await {
            result.extend(batch_result);
        }
    }
    result
}

/// Evaluates maintainers for a batch of attrpaths in a single `nix eval` call.
///
/// Each attr is wrapped in `builtins.tryEval` + `builtins.deepSeq` so that one
/// failing package does not abort the entire batch.
async fn eval_meta_batch(
    attrs: &[(String, String)], // (attrpath, nix_attr)
    commit: &str,
    is_nixos: bool,
) -> HashMap<String, MetaInfo> {
    let nix_file = if is_nixos {
        "nixpkgs/nixos/release-combined.nix"
    } else {
        "nixpkgs/pkgs/top-level/release.nix"
    };

    // Build one expression that imports nixpkgs once and evaluates all attrs.
    // `safe` wraps each access: deepSeq forces full evaluation before tryEval
    // catches any thrown errors, returning [] for failed attrs.
    let mut expr = format!(
        "let pkgs = import <{nix_file}> {{}};\n\
         safe = x: let r = builtins.tryEval (builtins.deepSeq x x); in if r.success then r.value else [];\n\
         in {{\n"
    );
    for (i, (_, nix_attr)) in attrs.iter().enumerate() {
        expr.push_str(&format!(
            "  \"{i}\" = safe (pkgs.{nix_attr}.meta.maintainers or []);\n"
        ));
    }
    expr.push('}');

    let nixpkgs_url = format!("nixpkgs=https://github.com/NixOS/nixpkgs/archive/{commit}.tar.gz");

    let output = Command::new("nix")
        .args(["eval", "--json", "--impure", "--expr", &expr])
        .env("NIX_PATH", &nixpkgs_url)
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            log::debug!(
                "nix eval batch failed ({} attrs): {}",
                attrs.len(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return HashMap::new();
        }
        Err(e) => {
            log::warn!("Could not spawn nix for batch ({} attrs): {e}", attrs.len());
            return HashMap::new();
        }
    };

    let Ok(val) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return HashMap::new();
    };

    attrs
        .iter()
        .enumerate()
        .map(|(i, (attrpath, _))| {
            let maintainers = val
                .get(i.to_string())
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("github")?.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            (attrpath.clone(), MetaInfo { maintainers })
        })
        .collect()
}
