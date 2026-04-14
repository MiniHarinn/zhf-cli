use anyhow::{Context, Result};
use zhf_types::FailureItem;

use crate::cli::{JobFilter, FailureFilter};

/// Base URL for the published GitHub Pages data.
/// Override at runtime with the ZHF_DATA_URL env variable (useful for local testing).
const DEFAULT_BASE_URL: &str = "https://moment.github.io/zhf";

fn base_url() -> String {
    std::env::var("ZHF_DATA_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

// Re-export the shared types so the rest of the crate can import from here
pub use zhf_types::IndexJson as Stats;

/// A failure item annotated with its kind for display purposes.
pub struct FailureEntry {
    pub item: FailureItem,
    pub kind: &'static str,
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

pub fn fetch_failures(job_filter: JobFilter, filter: &FailureFilter) -> Result<Vec<FailureEntry>> {
    // Determine which jobset files to load based on --nixpkgs / --nixos flags.
    // Loading only the needed file halves fetch time in the common filtered case.
    let jobsets: &[&str] = if filter.nixpkgs {
        &["nixpkgs"]
    } else if filter.nixos {
        &["nixos"]
    } else {
        &["nixpkgs", "nixos"]
    };

    let kinds: &[(&str, &'static str)] = match job_filter {
        JobFilter::Direct => &[("direct", "direct")],
        JobFilter::Indirect => &[("indirect", "indirect")],
        JobFilter::All => &[("direct", "direct"), ("indirect", "indirect")],
    };

    let mut entries = Vec::new();
    for jobset in jobsets {
        for (kind_slug, kind_label) in kinds {
            let path = format!("data/{kind_slug}_{jobset}.json");
            let items: Vec<FailureItem> = fetch_json(&path)?;
            entries.extend(items.into_iter().map(|item| FailureEntry { item, kind: kind_label }));
        }
    }
    Ok(entries)
}
