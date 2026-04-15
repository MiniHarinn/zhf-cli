use anyhow::{anyhow, Result};
use chrono::DateTime;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

pub struct EvalInfo {
    pub id: u64,
    pub time: String,
    pub nixpkgs_commit: String,
}

#[derive(Clone)]
pub struct Build {
    pub attrpath: String,
    // nix_attr has no "nixpkgs." prefix for nixpkgs builds
    pub nix_attr: String,
    pub platform: String,
    pub hydra_id: u64,
    pub status: BuildStatus,
    pub is_nixos: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BuildStatus {
    Direct,
    Indirect,
}

#[derive(Deserialize)]
struct HydraEval {
    id: u64,
    timestamp: u64,
    jobsetevalinputs: HashMap<String, HydraEvalInput>,
}

#[derive(Deserialize)]
struct HydraEvalInput {
    revision: Option<String>,
}

pub async fn get_latest_eval(client: &Client, project: &str, jobset: &str) -> Result<EvalInfo> {
    let url = format!("https://hydra.nixos.org/jobset/{project}/{jobset}/latest-eval");

    // latest-eval redirects to /eval/{id}; reqwest follows it automatically
    let text = fetch_with_retry(client, &url, "application/json").await?;
    let eval: HydraEval = serde_json::from_str(&text)?;

    let nixpkgs_commit = eval
        .jobsetevalinputs
        .get("nixpkgs")
        .and_then(|i| i.revision.clone())
        .ok_or_else(|| anyhow!("eval {} has no nixpkgs input", eval.id))?;

    Ok(EvalInfo {
        id: eval.id,
        time: format_timestamp(eval.timestamp),
        nixpkgs_commit,
    })
}

pub async fn get_eval_builds(client: &Client, eval_id: u64, is_nixos: bool) -> Result<Vec<Build>> {
    log::info!("Fetching builds for eval {eval_id} (is_nixos={is_nixos})…");

    // HTML endpoint — the JSON API (/eval/{id}/builds) times out on large evals
    let url = format!("https://hydra.nixos.org/eval/{eval_id}?full=1");
    let html = fetch_with_retry(client, &url, "text/html").await?;

    let builds = parse_eval_html(&html, is_nixos, eval_id)?;

    log::info!("Kept {} failed builds for eval {eval_id}", builds.len());
    Ok(builds)
}

async fn fetch_with_retry(client: &Client, url: &str, accept: &str) -> Result<String> {
    do_fetch(client, url, accept, 5).await
}

async fn fetch_with_throttled_retry(client: &Client, url: &str, accept: &str) -> Result<String> {
    do_fetch(client, url, accept, 8).await
}

async fn do_fetch(client: &Client, url: &str, accept: &str, max_attempts: u32) -> Result<String> {
    for attempt in 1..=max_attempts {
        let resp = client
            .get(url)
            .header("Accept", accept)
            .send()
            .await;

        match resp {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS && attempt < max_attempts => {
                // respect Retry-After header, fall back to exponential backoff starting at 5s
                let delay = r.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(5 * (1u64 << (attempt - 1).min(4)));
                log::warn!(
                    "Rate limited (attempt {attempt}/{max_attempts}) for {url}; retrying in {delay}s…"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
            Ok(r) => {
                let text = r.error_for_status()?.text().await?;
                return Ok(text);
            }
            Err(e) if attempt < max_attempts => {
                let delay = 1u64 << (attempt - 1).min(4);
                log::warn!(
                    "Request failed (attempt {attempt}/{max_attempts}): {e}; retrying in {delay}s…"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("all {max_attempts} attempts failed for {url}")
}

pub fn format_timestamp(ts: u64) -> String {
    DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S (UTC)")
        .to_string()
}

pub fn now_formatted() -> String {
    format_timestamp(chrono::Utc::now().timestamp() as u64)
}

fn parse_eval_html(html: &str, is_nixos: bool, eval_id: u64) -> Result<Vec<Build>> {
    let doc = Html::parse_document(html);

    // Hydra groups failures into tab panes: tabs-now-fail, tabs-still-fail, tabs-aborted
    let section_sel  = Selector::parse("#tabs-now-fail, #tabs-still-fail, #tabs-aborted").unwrap();
    let row_sel      = Selector::parse("tr").unwrap();
    let status_sel   = Selector::parse("img.build-status").unwrap();
    let link_sel     = Selector::parse("a[href*='/build/']").unwrap();
    let platform_sel = Selector::parse("td.nowrap tt").unwrap();

    let sections: Vec<_> = doc.select(&section_sel).collect();
    let rows: Vec<_> = if sections.is_empty() {
        // fallback: no tab panes found — parse entire document
        doc.root_element().select(&row_sel).collect()
    } else {
        sections.iter().flat_map(|s| s.select(&row_sel)).collect()
    };

    let mut builds = Vec::new();
    for row in rows {
        // build status is in img.build-status[title] (e.g. "Failed", "Dependency failed")
        let Some(status_text) = row
            .select(&status_sel)
            .next()
            .and_then(|e| e.value().attr("title"))
        else {
            continue;
        };
        let Some(status) = map_status(status_text) else {
            continue;
        };

        // each row has two /build/ links: links[0] = build ID, links[1] = job name
        let links: Vec<_> = row.select(&link_sel).collect();
        let Some(job_link) = links.get(1) else {
            continue;
        };

        let href = job_link.value().attr("href").unwrap_or("");
        let Some(hydra_id) = href.rsplit('/').next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };

        let job = job_link.text().collect::<String>();
        let job = job.trim();

        let Some(platform_el) = row.select(&platform_sel).next() else {
            continue;
        };
        let platform = platform_el.text().collect::<String>().trim().to_string();

        let (attrpath, nix_attr) = if is_nixos {
            if !job.starts_with("nixos.") {
                continue;
            }
            (job.to_string(), job.to_string())
        } else {
            (format!("nixpkgs.{job}"), job.to_string())
        };

        builds.push(Build {
            attrpath,
            nix_attr,
            platform,
            hydra_id,
            status,
            is_nixos,
        });
    }

    if builds.is_empty() {
        log::warn!("parsed zero failed builds for eval {eval_id} — eval may have no failures");
    }
    Ok(builds)
}

fn map_status(status: &str) -> Option<BuildStatus> {
    match status {
        "Failed" | "Failed with output" | "Timed out" | "Log limit exceeded"
        | "Output size limit exceeded" => Some(BuildStatus::Direct),
        "Dependency failed" => Some(BuildStatus::Indirect),
        _ => None,
    }
}

const DEFAULT_HYDRA_CONCURRENCY: usize = 4;
const PROGRESS_EVERY: usize = 500;

/// For each indirect build, resolve which direct build caused the failure.
/// Returns a map of indirect_hydra_id → causing_direct_hydra_id.
pub async fn resolve_failing_deps(
    client: &Client,
    indirect_builds: &[&Build],
) -> HashMap<u64, u64> {
    let concurrency = env::var("ZHF_HYDRA_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_HYDRA_CONCURRENCY);

    let total = indirect_builds.len();
    log::info!(
        "Resolving failing dependencies for {total} indirect builds (concurrency={concurrency})…"
    );

    let sem = Arc::new(Semaphore::new(concurrency));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    for build in indirect_builds {
        let client = client.clone();
        let hydra_id = build.hydra_id;
        let sem = sem.clone();
        let completed = completed.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let result = get_failing_dep_id(&client, hydra_id).await;
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            if done >= total || done % PROGRESS_EVERY == 0 {
                log::info!("Dependency resolution: {done}/{total}");
            }
            match result {
                Ok(Some(dep_id)) => Some((hydra_id, dep_id)),
                Ok(None) => None,
                Err(e) => {
                    log::debug!("Failed to resolve dep for build {hydra_id}: {e}");
                    None
                }
            }
        }));
    }

    let mut map = HashMap::new();
    for handle in handles {
        if let Ok(Some((indirect_id, direct_id))) = handle.await {
            map.insert(indirect_id, direct_id);
        }
    }

    log::info!("Resolved {} / {} dependency links", map.len(), total);
    map
}

/// Fetch a single build page from Hydra and extract the build ID of the
/// dependency that caused the failure (from the "propagated from" link).
async fn get_failing_dep_id(client: &Client, build_id: u64) -> Result<Option<u64>> {
    let url = format!("https://hydra.nixos.org/build/{build_id}");
    let html = fetch_with_throttled_retry(client, &url, "text/html").await?;

    let doc = Html::parse_document(&html);
    let sel = Selector::parse("a[href*='/build/']").unwrap();

    // Look for "(propagated from <a href='.../build/NNN'>build NNN</a>)" links
    // in the "Failed build steps" section
    for el in doc.select(&sel) {
        let text = el.text().collect::<String>();
        if !text.starts_with("build ") {
            continue;
        }
        // Check that the parent text contains "propagated from"
        if let Some(parent) = el.parent() {
            let parent_text: String = parent
                .children()
                .filter_map(|c| c.value().as_text().map(|t| t.to_string()))
                .collect();
            if !parent_text.contains("propagated from") {
                continue;
            }
        }
        if let Some(href) = el.value().attr("href") {
            if let Some(id_str) = href.rsplit('/').next() {
                if let Ok(dep_id) = id_str.parse::<u64>() {
                    return Ok(Some(dep_id));
                }
            }
        }
    }

    Ok(None)
}
