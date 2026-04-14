use anyhow::{anyhow, Result};
use chrono::DateTime;
use reqwest::Client;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::collections::HashMap;

pub struct EvalInfo {
    pub id: u64,
    pub time: String,
    pub nixpkgs_commit: String,
}

pub struct Build {
    /// Display attrpath (e.g. "nixos.tests.foo.x86_64-linux" or "nixpkgs.bar.x86_64-linux")
    pub attrpath: String,
    /// Attribute to pass to `nix eval` (no "nixpkgs." prefix for nixpkgs builds)
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

// ── Hydra JSON shapes ──────────────────────────────────────────────────────

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

// ── Public API ─────────────────────────────────────────────────────────────

/// Returns the latest *finished* eval for a jobset, along with its nixpkgs commit.
///
/// Uses `/jobset/{project}/{jobset}/latest-eval` which Hydra guarantees points
/// to the most recently completed evaluation.
pub async fn get_latest_eval(client: &Client, project: &str, jobset: &str) -> Result<EvalInfo> {
    let url = format!("https://hydra.nixos.org/jobset/{project}/{jobset}/latest-eval");

    // latest-eval returns a 302. reqwest follows redirects by default, so we
    // land on /eval/{id} and get the JSON straight away.
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

/// Fetches all failed builds for an evaluation by parsing Hydra's eval HTML.
///
/// * `is_nixos = true`  → nixos/unstable eval; only `nixos.*` jobs are kept.
/// * `is_nixos = false` → nixpkgs/unstable eval; all jobs kept, `nixpkgs.` prepended.
///
/// Uses the HTML endpoint (`/eval/{id}?full=1`) rather than the JSON API
/// (`/eval/{id}/builds`) because the JSON endpoint times out on large evals.
pub async fn get_eval_builds(client: &Client, eval_id: u64, is_nixos: bool) -> Result<Vec<Build>> {
    log::info!("Fetching builds for eval {eval_id} (is_nixos={is_nixos})…");

    let url = format!("https://hydra.nixos.org/eval/{eval_id}?full=1");
    let html = fetch_with_retry(client, &url, "text/html").await?;

    let builds = parse_eval_html(&html, is_nixos, eval_id)?;

    log::info!("Kept {} failed builds for eval {eval_id}", builds.len());
    Ok(builds)
}

// ── Retry helper ──────────────────────────────────────────────────────────

/// Fetch a URL, retrying up to 5 times with exponential back-off (1 s, 2 s, 4 s, …).
/// Returns the response body as a `String`.
async fn fetch_with_retry(client: &Client, url: &str, accept: &str) -> Result<String> {
    const MAX_ATTEMPTS: u32 = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        let result: std::result::Result<String, reqwest::Error> = async {
            client
                .get(url)
                .header("Accept", accept)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await
        }
        .await;

        match result {
            Ok(text) => return Ok(text),
            Err(e) if attempt < MAX_ATTEMPTS => {
                let delay = 1u64 << (attempt - 1); // 1, 2, 4, 8 s
                log::warn!(
                    "Request failed (attempt {attempt}/{MAX_ATTEMPTS}): {e}; retrying in {delay}s…"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}

// ── Helpers ────────────────────────────────────────────────────────────────

pub fn format_timestamp(ts: u64) -> String {
    DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S (UTC)")
        .to_string()
}

/// Formats the current UTC time using the same format as `format_timestamp`.
pub fn now_formatted() -> String {
    format_timestamp(chrono::Utc::now().timestamp() as u64)
}

fn parse_eval_html(html: &str, is_nixos: bool, eval_id: u64) -> Result<Vec<Build>> {
    let doc = Html::parse_document(html);

    // The Hydra eval page organises builds into named tab panes:
    // "tabs-now-fail" (new regressions), "tabs-still-fail" (chronic), "tabs-aborted".
    let section_sel  = Selector::parse("#tabs-now-fail, #tabs-still-fail, #tabs-aborted").unwrap();
    let row_sel      = Selector::parse("tr").unwrap();
    let status_sel   = Selector::parse("img.build-status").unwrap();
    let link_sel     = Selector::parse("a[href*='/build/']").unwrap();
    let platform_sel = Selector::parse("td.nowrap tt").unwrap();

    let sections: Vec<_> = doc.select(&section_sel).collect();
    let rows: Vec<_> = if sections.is_empty() {
        // Fallback: parse whole document (no section divs — test HTML or Hydra changed its structure).
        doc.root_element().select(&row_sel).collect()
    } else {
        sections.iter().flat_map(|s| s.select(&row_sel)).collect()
    };

    let mut builds = Vec::new();
    for row in rows {
        // Status is encoded in an img title attribute (e.g. "Failed", "Dependency failed").
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

        // Each row has two build links: links[0] = row-link (text = build ID number),
        // links[1] = job name link. Both href to /build/{id}.
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
