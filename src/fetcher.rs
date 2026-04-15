use anyhow::{Context, Result};
use zhf_types::FailureItem;

use crate::cli::{FailureFilter, JobFilter};

/// Base URL for the published GitHub Pages data.
/// Override at runtime with the ZHF_DATA_URL env variable (useful for local testing).
const DEFAULT_BASE_URL: &str = "https://zhf.harinn.dev";

fn base_url() -> String {
    std::env::var("ZHF_DATA_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

pub use zhf_types::IndexJson as Stats;

pub struct FailureEntry {
    pub item: FailureItem,
    pub kind: &'static str,
    pub channel: String,
}

fn fetch_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T> {
    let url = format!("{}/{path}", base_url());
    let client = reqwest::blocking::Client::builder()
        .user_agent("zhf-cli/0.1")
        .build()?;
    let value = client
        .get(&url)
        .send()
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error for {url}"))?
        .json::<T>()
        .with_context(|| format!("parsing JSON from {url}"))?;
    Ok(value)
}

pub fn fetch_stats() -> Result<Stats> {
    fetch_json("data/index.json")
}

fn channel_to_slug(spec: &str) -> Option<&'static str> {
    match spec {
        "nixos:unstable"      => Some("nixos_unstable"),
        "nixos:staging"       => Some("nixos_staging"),
        "nixpkgs:unstable"    => Some("nixpkgs_unstable"),
        "nixpkgs:staging-next" => Some("nixpkgs_staging_next"),
        other => {
            eprintln!("warning: unknown channel {other:?} — valid values: nixos:unstable, nixos:staging, nixpkgs:unstable, nixpkgs:staging-next");
            None
        }
    }
}

pub fn fetch_failures(job_filter: JobFilter, filter: &FailureFilter) -> Result<Vec<FailureEntry>> {
    let slugs: Vec<(&'static str, String)> = filter.channel
        .iter()
        .filter_map(|spec| channel_to_slug(spec).map(|slug| (slug, spec.clone())))
        .collect();

    let kinds: &[(&str, &'static str)] = match job_filter {
        JobFilter::Direct   => &[("direct", "direct")],
        JobFilter::Indirect => &[("indirect", "indirect")],
        JobFilter::All      => &[("direct", "direct"), ("indirect", "indirect")],
    };

    let mut entries = Vec::new();
    for (slug, channel) in slugs {
        for (kind_slug, kind_label) in kinds {
            let path = format!("data/{kind_slug}_{slug}.json");
            let items: Vec<FailureItem> = fetch_json(&path)?;
            entries.extend(items.into_iter().map(|item| FailureEntry {
                item,
                kind: kind_label,
                channel: channel.clone(),
            }));
        }
    }
    Ok(entries)
}
