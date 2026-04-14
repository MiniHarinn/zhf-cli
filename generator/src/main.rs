mod hydra;
mod maintainers;

use anyhow::Result;
use hydra::BuildStatus;
use std::collections::HashMap;
use std::fs;
use zhf_types::{EvalInfo, FailureCounts, FailureItem, IndexJson};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp(None)
        .init();

    let client = reqwest::Client::builder()
        .user_agent("zhf-generator/0.1 (github.com/moment/zhf)")
        // Keep TCP connections alive so the OS doesn't kill them while Hydra
        // serializes large eval responses (can take minutes for 280k builds).
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .connection_verbose(true)
        .build()?;

    // ── 1. Fetch latest finished eval for each jobset ──────────────────────
    log::info!("Fetching latest evals…");
    let (nixos_eval, nixpkgs_eval) = tokio::try_join!(
        hydra::get_latest_eval(&client, "nixos", "unstable"),
        hydra::get_latest_eval(&client, "nixpkgs", "unstable"),
    )?;
    log::info!(
        "nixos/unstable eval: {} | nixpkgs/unstable eval: {}",
        nixos_eval.id,
        nixpkgs_eval.id
    );

    // ── 2. Fetch builds from both evals ────────────────────────────────────
    log::info!("Fetching builds…");
    let nixos_builds = hydra::get_eval_builds(&client, nixos_eval.id, true).await?;
    let nixpkgs_builds = hydra::get_eval_builds(&client, nixpkgs_eval.id, false).await?;

    let all_builds: Vec<_> = nixos_builds.into_iter().chain(nixpkgs_builds).collect();
    log::info!("Total failed builds: {}", all_builds.len());

    // ── 3. Resolve maintainers ─────────────────────────────────────────────
    log::info!("Resolving maintainers (this may take a while)…");
    let maintainers_map = maintainers::resolve_all(
        &all_builds,
        &nixos_eval.nixpkgs_commit,
        &nixpkgs_eval.nixpkgs_commit,
    )
    .await;
    log::info!("Resolved maintainers for {} builds", maintainers_map.len());

    // ── 4. Build output JSON ───────────────────────────────────────────────
    let mut counts = FailureCounts {
        aarch64_darwin: 0,
        aarch64_linux: 0,
        x86_64_darwin: 0,
        x86_64_linux: 0,
        i686_linux: 0,
        total: 0,
    };

    // Dedup by attrpath — prefer direct over indirect
    let mut seen: HashMap<&str, BuildStatus> = HashMap::new();
    for b in &all_builds {
        let entry = seen.entry(b.attrpath.as_str()).or_insert(b.status);
        if b.status == BuildStatus::Direct {
            *entry = BuildStatus::Direct;
        }
    }

    let mut direct: Vec<FailureItem> = Vec::new();
    let mut indirect: Vec<FailureItem> = Vec::new();

    for b in &all_builds {
        // If this attrpath is known as direct, skip its indirect entry
        if seen.get(b.attrpath.as_str()) == Some(&BuildStatus::Direct)
            && b.status == BuildStatus::Indirect
        {
            continue;
        }
        // Skip duplicate attrpath entries (same attrpath, same status)
        // We already have a canonical entry via `seen` — only emit once
        if seen.remove(b.attrpath.as_str()).is_none() {
            continue;
        }

        // Update platform counts
        match b.platform.as_str() {
            "aarch64-darwin" => counts.aarch64_darwin += 1,
            "aarch64-linux" => counts.aarch64_linux += 1,
            "x86_64-darwin" => counts.x86_64_darwin += 1,
            "x86_64-linux" => counts.x86_64_linux += 1,
            "i686-linux" => counts.i686_linux += 1,
            _ => {}
        }
        counts.total += 1;

        let maintainers = maintainers_map
            .get(&b.attrpath)
            .cloned()
            .unwrap_or_default();

        let item = FailureItem {
            attrpath: b.attrpath.clone(),
            platform: b.platform.clone(),
            maintainers,
            hydra_url: format!("https://hydra.nixos.org/build/{}", b.hydra_id),
        };

        match b.status {
            BuildStatus::Direct => direct.push(item),
            BuildStatus::Indirect => indirect.push(item),
        }
    }

    let generated_at = {
        // Current time via system clock (no external dep)
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        hydra::format_timestamp(secs)
    };

    let index = IndexJson {
        generated_at,
        nixos_eval: EvalInfo {
            id: nixos_eval.id,
            time: nixos_eval.time,
        },
        nixpkgs_eval: EvalInfo {
            id: nixpkgs_eval.id,
            time: nixpkgs_eval.time,
        },
        counts,
    };

    // ── 5. Write output files ──────────────────────────────────────────────
    fs::create_dir_all("output/data")?;

    fs::write(
        "output/data/index.json",
        serde_json::to_string_pretty(&index)?,
    )?;
    fs::write(
        "output/data/direct.json",
        serde_json::to_string_pretty(&direct)?,
    )?;
    fs::write(
        "output/data/indirect.json",
        serde_json::to_string_pretty(&indirect)?,
    )?;

    log::info!(
        "Done. direct={} indirect={} total={}",
        direct.len(),
        indirect.len(),
        index.counts.total
    );
    Ok(())
}
