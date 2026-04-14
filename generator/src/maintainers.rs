use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::hydra::Build;

/// Max parallel `nix eval` processes. Each process now evaluates a batch of attrs.
const DEFAULT_PARALLEL_NIX_EVALS: usize = 2;
/// Number of attrpaths to resolve in a single `nix eval` invocation.
const BATCH_SIZE: usize = 10;
/// Number of completed batches between progress updates.
const PROGRESS_REPORT_EVERY_BATCHES: usize = 100;

/// Package metadata resolved via `nix eval`.
#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

/// Resolves package metadata for all failed builds in parallel.
///
/// All builds in a single call share the same nixpkgs `commit` (one per channel).
/// Deduplicates by attrpath, then batches multiple attrpaths into a single `nix eval`
/// call to amortize nixpkgs import cost.
///
/// Returns a map of `attrpath → MetaInfo`.
pub async fn resolve_all(builds: &[Build], commit: &str) -> HashMap<String, MetaInfo> {
    let parallel_nix_evals = maintainer_eval_concurrency();

    // Deduplicate: one nix eval per unique attrpath (maintainers are per-package, not per-platform)
    let mut unique: HashMap<&str, (&str, bool)> = HashMap::new();
    for build in builds {
        unique
            .entry(build.attrpath.as_str())
            .or_insert((build.nix_attr.as_str(), build.is_nixos));
    }

    // Group by is_nixos so each batch shares a single nixpkgs import (nix_file differs)
    let mut groups: HashMap<bool, Vec<(&str, &str)>> = HashMap::new();
    for (attrpath, (nix_attr, is_nixos)) in &unique {
        groups
            .entry(*is_nixos)
            .or_default()
            .push((attrpath, nix_attr));
    }

    let total = unique.len();
    let completed = Arc::new(AtomicUsize::new(0));
    let completed_batches = Arc::new(AtomicUsize::new(0));
    let sem = Arc::new(Semaphore::new(parallel_nix_evals));
    let mut handles = Vec::new();

    log::info!(
        "Resolving maintainers with concurrency={} batch_size={}",
        parallel_nix_evals,
        BATCH_SIZE
    );

    for (is_nixos, attrs) in groups {
        for chunk in attrs.chunks(BATCH_SIZE) {
            let chunk: Vec<(String, String)> = chunk
                .iter()
                .map(|(ap, na)| (ap.to_string(), na.to_string()))
                .collect();
            let commit = commit.to_string();
            let sem = sem.clone();
            let completed = completed.clone();
            let completed_batches = completed_batches.clone();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.ok()?;
                let result = eval_meta_batch(&chunk, &commit, is_nixos).await;
                let done = completed.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                let batch = completed_batches.fetch_add(1, Ordering::Relaxed) + 1;
                if done >= total || batch % PROGRESS_REPORT_EVERY_BATCHES == 0 {
                    log::info!("Maintainers: {done}/{total} attrs resolved");
                }
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
    // release-combined.nix explicitly strips meta.maintainers via removeMaintainers;
    // use release.nix directly instead, which preserves them.
    // release.nix exposes jobs without the "nixos." prefix, so strip it from nix_attr.
    let nix_file = if is_nixos {
        "nixpkgs/nixos/release.nix"
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
        let attr = if is_nixos {
            nix_attr.strip_prefix("nixos.").unwrap_or(nix_attr)
        } else {
            nix_attr.as_str()
        };
        expr.push_str(&format!(
            "  \"{i}\" = safe (pkgs.{attr}.meta.maintainers or []);\n"
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

fn maintainer_eval_concurrency() -> usize {
    env::var("ZHF_MAINTAINER_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PARALLEL_NIX_EVALS)
}
