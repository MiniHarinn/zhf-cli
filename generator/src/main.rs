mod hydra;
mod maintainers;

use anyhow::Result;
use hydra::BuildStatus;
use std::collections::{HashMap, HashSet};
use std::fs;
use zhf_types::{ChannelInfo, EvalInfo, FailureCounts, FailureItem, IndexJson};

struct ChannelSpec {
    project: &'static str,
    jobset: &'static str,
    slug: &'static str,
    is_nixos: bool,
}

const CHANNELS: &[ChannelSpec] = &[
    ChannelSpec { project: "nixos",   jobset: "unstable",     slug: "nixos_unstable",       is_nixos: true  },
    ChannelSpec { project: "nixos",   jobset: "staging",      slug: "nixos_staging",        is_nixos: true  },
    ChannelSpec { project: "nixpkgs", jobset: "unstable",     slug: "nixpkgs_unstable",     is_nixos: false },
    ChannelSpec { project: "nixpkgs", jobset: "staging-next", slug: "nixpkgs_staging_next", is_nixos: false },
];

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
        .build()?;

    // ── 1. Fetch latest finished eval for all channels ─────────────────────
    log::info!("Fetching latest evals for {} channels…", CHANNELS.len());
    let evals: Vec<hydra::EvalInfo> = futures::future::try_join_all(
        CHANNELS.iter().map(|ch| hydra::get_latest_eval(&client, ch.project, ch.jobset))
    ).await?;
    for (ch, eval) in CHANNELS.iter().zip(&evals) {
        log::info!("{}/{} eval: {}", ch.project, ch.jobset, eval.id);
    }

    // ── 2. Fetch builds for all channels ───────────────────────────────────
    log::info!("Fetching builds for all channels…");
    let builds_per_channel: Vec<Vec<hydra::Build>> = futures::future::try_join_all(
        CHANNELS.iter().zip(&evals).map(|(ch, eval)| {
            hydra::get_eval_builds(&client, eval.id, ch.is_nixos)
        })
    ).await?;

    fs::create_dir_all("output/data")?;

    // ── 3. Per-channel: resolve maintainers, categorize, write files ───────
    let mut channel_index: HashMap<String, ChannelInfo> = HashMap::new();

    for (i, ch) in CHANNELS.iter().enumerate() {
        let eval = &evals[i];
        let builds = &builds_per_channel[i];

        log::info!(
            "Resolving maintainers for {}/{} ({} builds)…",
            ch.project, ch.jobset, builds.len()
        );
        let maintainers_map = maintainers::resolve_all(builds, &eval.nixpkgs_commit).await;

        let (direct, indirect, direct_counts, indirect_counts) =
            categorize_builds(builds, &maintainers_map);

        log::info!(
            "{}/{}: direct={} indirect={}",
            ch.project, ch.jobset, direct.len(), indirect.len()
        );

        fs::write(
            format!("output/data/direct_{}.json", ch.slug),
            serde_json::to_string_pretty(&direct)?,
        )?;
        fs::write(
            format!("output/data/indirect_{}.json", ch.slug),
            serde_json::to_string_pretty(&indirect)?,
        )?;

        channel_index.insert(
            ch.slug.to_string(),
            ChannelInfo {
                eval: EvalInfo { id: eval.id, time: eval.time.clone() },
                direct_counts,
                indirect_counts,
            },
        );
    }

    // ── 4. Write index.json ────────────────────────────────────────────────
    let index = IndexJson {
        generated_at: hydra::now_formatted(),
        channels: channel_index,
    };
    fs::write("output/data/index.json", serde_json::to_string_pretty(&index)?)?;

    log::info!("Done.");
    Ok(())
}

/// Deduplicates builds by attrpath (preferring Direct over Indirect) and
/// categorizes them into direct/indirect lists with per-kind platform counts.
fn categorize_builds(
    builds: &[hydra::Build],
    maintainers_map: &HashMap<String, maintainers::MetaInfo>,
) -> (Vec<FailureItem>, Vec<FailureItem>, FailureCounts, FailureCounts) {
    let mut direct_counts = FailureCounts::default();
    let mut indirect_counts = FailureCounts::default();

    // Pass 1: collect attrpaths that have at least one Direct failure.
    let has_direct: HashSet<&str> = builds
        .iter()
        .filter(|b| b.status == BuildStatus::Direct)
        .map(|b| b.attrpath.as_str())
        .collect();

    // Pass 2: emit each attrpath once, skipping Indirect when a Direct exists.
    let mut emitted: HashSet<&str> = HashSet::new();
    let mut direct_items: Vec<FailureItem> = Vec::new();
    let mut indirect_items: Vec<FailureItem> = Vec::new();

    for b in builds {
        if b.status == BuildStatus::Indirect && has_direct.contains(b.attrpath.as_str()) {
            continue;
        }
        if !emitted.insert(b.attrpath.as_str()) {
            continue;
        }

        let counts = if b.status == BuildStatus::Direct {
            &mut direct_counts
        } else {
            &mut indirect_counts
        };
        match b.platform.as_str() {
            "aarch64-darwin" => counts.aarch64_darwin += 1,
            "aarch64-linux"  => counts.aarch64_linux += 1,
            "x86_64-darwin"  => counts.x86_64_darwin += 1,
            "x86_64-linux"   => counts.x86_64_linux += 1,
            "i686-linux"     => counts.i686_linux += 1,
            _ => {}
        }
        counts.total += 1;

        let meta = maintainers_map
            .get(&b.attrpath)
            .cloned()
            .unwrap_or_default();

        let item = FailureItem {
            attrpath: b.attrpath.clone(),
            platform: b.platform.clone(),
            maintainers: meta.maintainers,
            hydra_id: b.hydra_id,
        };

        match b.status {
            BuildStatus::Direct   => direct_items.push(item),
            BuildStatus::Indirect => indirect_items.push(item),
        }
    }

    (direct_items, indirect_items, direct_counts, indirect_counts)
}
