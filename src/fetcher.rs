use anyhow::{Context, Result};
use zhf_types::FailureItem;

use crate::cli::JobFilter;

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

pub fn fetch_failures(job_filter: JobFilter) -> Result<Vec<FailureEntry>> {
    match job_filter {
        JobFilter::Direct => {
            let items: Vec<FailureItem> = fetch_json("data/direct.json")?;
            Ok(items.into_iter().map(|item| FailureEntry { item, kind: "direct" }).collect())
        }
        JobFilter::Indirect => {
            let items: Vec<FailureItem> = fetch_json("data/indirect.json")?;
            Ok(items.into_iter().map(|item| FailureEntry { item, kind: "indirect" }).collect())
        }
        JobFilter::All => {
            let direct: Vec<FailureItem> = fetch_json("data/direct.json")?;
            let indirect: Vec<FailureItem> = fetch_json("data/indirect.json")?;
            let mut entries: Vec<FailureEntry> = direct
                .into_iter()
                .map(|item| FailureEntry { item, kind: "direct" })
                .collect();
            entries.extend(indirect.into_iter().map(|item| FailureEntry { item, kind: "indirect" }));
            Ok(entries)
        }
    }
}
