mod feed;
mod hydra;
mod maintainers;

use anyhow::Result;
use hydra::BuildStatus;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use zhf_types::{ChannelInfo, EvalInfo, FailureCounts, FailureItem, IndexJson, ProblematicItem};

const DEFAULT_PUBLIC_BASE_URL: &str = "https://zhf.harinn.dev";

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
        // keepalive prevents the OS from dropping the connection while Hydra serializes large evals
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .build()?;

    log::info!("Fetching latest evals for {} channels…", CHANNELS.len());
    let evals: Vec<hydra::EvalInfo> = futures::future::try_join_all(
        CHANNELS.iter().map(|ch| hydra::get_latest_eval(&client, ch.project, ch.jobset))
    ).await?;
    for (ch, eval) in CHANNELS.iter().zip(&evals) {
        log::info!("{}/{} eval: {}", ch.project, ch.jobset, eval.id);
    }

    log::info!("Fetching builds for all channels…");
    let builds_per_channel: Vec<Vec<hydra::Build>> = futures::future::try_join_all(
        CHANNELS.iter().zip(&evals).map(|(ch, eval)| {
            hydra::get_eval_builds(&client, eval.id, ch.is_nixos)
        })
    ).await?;

    fs::create_dir_all("output/data")?;

    // Deduplicate attrs across channels and resolve maintainers once per unique attr.
    // evals[0] = nixos/unstable, evals[2] = nixpkgs/unstable (most up-to-date commits).
    let nixos_unstable_commit = &evals[0].nixpkgs_commit;
    let nixpkgs_unstable_commit = &evals[2].nixpkgs_commit;

    let mut seen_nixos: HashSet<String> = HashSet::new();
    let mut seen_nixpkgs: HashSet<String> = HashSet::new();
    let mut global_nixos: Vec<hydra::Build> = Vec::new();
    let mut global_nixpkgs: Vec<hydra::Build> = Vec::new();

    for builds in &builds_per_channel {
        for b in builds {
            if b.is_nixos {
                if seen_nixos.insert(b.nix_attr.clone()) {
                    global_nixos.push(b.clone());
                }
            } else if seen_nixpkgs.insert(b.nix_attr.clone()) {
                global_nixpkgs.push(b.clone());
            }
        }
    }

    log::info!(
        "Resolving maintainers: {} unique nixos attrs, {} unique nixpkgs attrs (concurrent)…",
        global_nixos.len(),
        global_nixpkgs.len()
    );

    let sem = maintainers::make_semaphore();
    let (nixos_maintainers, nixpkgs_maintainers) = tokio::join!(
        maintainers::resolve_all(&global_nixos, nixos_unstable_commit, sem.clone()),
        maintainers::resolve_all(&global_nixpkgs, nixpkgs_unstable_commit, sem.clone()),
    );

    let mut all_maintainers = nixos_maintainers;
    all_maintainers.extend(nixpkgs_maintainers);
    log::info!("Maintainers resolved for {} unique attrs total", all_maintainers.len());

    let mut channel_index: HashMap<String, ChannelInfo> = HashMap::new();
    // Flat list of (channel_slug, item, kind) across all channels — consumed by
    // the feed pipeline after the per-channel loop finishes.
    let mut all_current: Vec<(String, FailureItem, &'static str)> = Vec::new();

    for (i, ch) in CHANNELS.iter().enumerate() {
        let builds = &builds_per_channel[i];

        let (direct, indirect, direct_counts, indirect_counts) =
            categorize_builds(builds, &all_maintainers);

        log::info!(
            "{}/{}: direct={} indirect={}",
            ch.project, ch.jobset, direct.len(), indirect.len()
        );

        let indirect_builds: Vec<&hydra::Build> = builds
            .iter()
            .filter(|b| b.status == BuildStatus::Indirect)
            .collect();
        let dep_map = hydra::resolve_failing_deps(&client, &indirect_builds).await;
        let problematic = aggregate_problematic(&dep_map, builds, &all_maintainers);
        log::info!("{}/{}: problematic={}", ch.project, ch.jobset, problematic.len());

        fs::write(
            format!("output/data/direct_{}.json", ch.slug),
            serde_json::to_string_pretty(&direct)?,
        )?;
        fs::write(
            format!("output/data/indirect_{}.json", ch.slug),
            serde_json::to_string_pretty(&indirect)?,
        )?;
        fs::write(
            format!("output/data/problematic_{}.json", ch.slug),
            serde_json::to_string_pretty(&problematic)?,
        )?;

        let eval = &evals[i];
        channel_index.insert(
            ch.slug.to_string(),
            ChannelInfo {
                eval: EvalInfo { id: eval.id, time: eval.time.clone() },
                direct_counts,
                indirect_counts,
                problematic_count: problematic.len() as u32,
            },
        );

        for item in direct {
            all_current.push((ch.slug.to_string(), item, "direct"));
        }
        for item in indirect {
            all_current.push((ch.slug.to_string(), item, "indirect"));
        }
    }

    let index = IndexJson {
        generated_at: hydra::now_formatted(),
        channels: channel_index,
    };
    fs::write("output/data/index.json", serde_json::to_string_pretty(&index)?)?;

    generate_feeds(&client, &all_current).await?;

    log::info!("Done.");
    Ok(())
}

async fn generate_feeds(
    client: &reqwest::Client,
    all_current: &[(String, FailureItem, &'static str)],
) -> Result<()> {
    let base_url = std::env::var("ZHF_PUBLIC_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_PUBLIC_BASE_URL.to_string());

    let prev_state = feed::load_previous_state(client, &base_url).await?;

    let current: Vec<feed::CurrentFailure> = all_current
        .iter()
        .map(|(slug, item, kind)| feed::CurrentFailure {
            channel_slug: slug.as_str(),
            item,
            kind,
        })
        .collect();

    let now = chrono::Utc::now();
    let (state, new_keys) = feed::compute_next_state(prev_state.as_ref(), &current, now);
    log::info!(
        "Feed: {} currently-failing entries, {} newly failing since last run",
        state.failures.len(),
        new_keys.len()
    );

    let output_dir = Path::new("output");
    feed::write_state(output_dir, &state)?;

    let channel_lookup: HashMap<String, feed::ChannelDisplay> = CHANNELS
        .iter()
        .map(|ch| {
            (
                ch.slug.to_string(),
                feed::ChannelDisplay {
                    project: ch.project.to_string(),
                    jobset: ch.jobset.to_string(),
                },
            )
        })
        .collect();

    feed::write_feeds(output_dir, &state, &base_url, &channel_lookup, now)?;
    Ok(())
}

fn aggregate_problematic(
    dep_map: &HashMap<u64, u64>,
    builds: &[hydra::Build],
    maintainers_map: &HashMap<String, maintainers::MetaInfo>,
) -> Vec<ProblematicItem> {
    // Build lookup: hydra_id → Build for direct builds
    let direct_by_id: HashMap<u64, &hydra::Build> = builds
        .iter()
        .filter(|b| b.status == BuildStatus::Direct)
        .map(|b| (b.hydra_id, b))
        .collect();

    // Group indirect builds by their causing direct build ID
    let mut groups: HashMap<u64, Vec<&hydra::Build>> = HashMap::new();
    for b in builds.iter().filter(|b| b.status == BuildStatus::Indirect) {
        if let Some(&direct_id) = dep_map.get(&b.hydra_id) {
            if direct_by_id.contains_key(&direct_id) {
                groups.entry(direct_id).or_default().push(b);
            }
        }
    }

    let mut items: Vec<ProblematicItem> = groups
        .into_iter()
        .filter_map(|(direct_id, blocked_builds)| {
            let direct = direct_by_id.get(&direct_id)?;
            let meta = maintainers_map
                .get(&direct.attrpath)
                .cloned()
                .unwrap_or_default();

            let mut blocked: Vec<String> = blocked_builds
                .iter()
                .map(|b| b.attrpath.clone())
                .collect();
            blocked.sort();
            blocked.dedup();

            Some(ProblematicItem {
                attrpath: direct.attrpath.clone(),
                platform: direct.platform.clone(),
                maintainers: meta.maintainers,
                hydra_id: direct.hydra_id,
                blocked_count: blocked.len() as u32,
                blocked,
            })
        })
        .collect();

    items.sort_by(|a, b| b.blocked_count.cmp(&a.blocked_count));
    items
}

fn categorize_builds(
    builds: &[hydra::Build],
    maintainers_map: &HashMap<String, maintainers::MetaInfo>,
) -> (Vec<FailureItem>, Vec<FailureItem>, FailureCounts, FailureCounts) {
    let mut direct_counts = FailureCounts::default();
    let mut indirect_counts = FailureCounts::default();

    // pass 1: find all attrs that have at least one direct failure
    let has_direct: HashSet<&str> = builds
        .iter()
        .filter(|b| b.status == BuildStatus::Direct)
        .map(|b| b.attrpath.as_str())
        .collect();

    // pass 2: emit each attr once, skipping indirect when a direct exists
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
