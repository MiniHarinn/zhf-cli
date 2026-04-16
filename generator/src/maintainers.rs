use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::hydra::Build;

const DEFAULT_PARALLEL_NIX_EVALS: usize = 2; // max concurrent nix eval processes
const BATCH_SIZE: usize = 100;                // attrs per nix eval invocation
const PROGRESS_REPORT_EVERY_BATCHES: usize = 100;

#[derive(Default, Clone)]
pub struct MetaInfo {
    pub maintainers: Vec<String>,
}

pub async fn resolve_all(
    builds: &[Build],
    commit: &str,
    sem: Arc<Semaphore>,
) -> HashMap<String, MetaInfo> {
    // maintainers are per-package, not per-platform — deduplicate by attrpath
    let mut unique: HashMap<&str, (&str, bool)> = HashMap::new();
    for build in builds {
        unique
            .entry(build.attrpath.as_str())
            .or_insert((build.nix_attr.as_str(), build.is_nixos));
    }

    // group by type so each batch shares one nixpkgs import (nix_file differs between nixos/nixpkgs)
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
    let mut handles = Vec::new();

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

async fn eval_meta_batch(
    attrs: &[(String, String)],
    commit: &str,
    is_nixos: bool,
) -> HashMap<String, MetaInfo> {
    // release-combined.nix strips maintainers via removeMaintainers; use release.nix instead
    let nix_file = if is_nixos {
        "nixpkgs/nixos/release.nix"
    } else {
        "nixpkgs/pkgs/top-level/release.nix"
    };

    // import nixpkgs once, eval all attrs; `safe` uses tryEval+deepSeq to isolate per-attr failures
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

/// Build the shared semaphore that bounds total concurrent `nix eval` processes
/// across all `resolve_all` calls. Must be shared (not recreated per call) so
/// that `ZHF_MAINTAINER_CONCURRENCY` reflects true process-level concurrency.
pub fn make_semaphore() -> Arc<Semaphore> {
    let n = maintainer_eval_concurrency();
    log::info!("Resolving maintainers with concurrency={n} batch_size={BATCH_SIZE}");
    Arc::new(Semaphore::new(n))
}
