use anyhow::{anyhow, Result};
use chrono::DateTime;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

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
    let eval: HydraEval = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

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

    let html = retry(5, Duration::from_secs(30), || async {
        client
            .get(format!("https://hydra.nixos.org/eval/{eval_id}?full=1"))
            .header("Accept", "text/html")
            .timeout(Duration::from_secs(300))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await
    })
    .await?;

    let builds = parse_eval_html(&html, is_nixos, eval_id)?;

    log::info!(
        "Kept {} failed builds for eval {eval_id}",
        builds.len()
    );
    Ok(builds)
}

// ── Helpers ────────────────────────────────────────────────────────────────

pub fn format_timestamp(ts: u64) -> String {
    DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S (UTC)")
        .to_string()
}

fn parse_eval_html(html: &str, is_nixos: bool, eval_id: u64) -> Result<Vec<Build>> {
    let mut builds = Vec::new();

    for row in extract_all(html, "<tr>", "</tr>") {
        let Some(status_text) = extract_status(row) else {
            continue;
        };
        let Some(status) = map_status(&status_text) else {
            continue;
        };

        let links = extract_build_links(row);
        // links[0]: row-link (text = build ID number), links[1]: job name link
        let Some((hydra_id, job_name)) = links.get(1).cloned() else {
            continue;
        };
        let Some(platform) = extract_between(row, "<td class=\"nowrap\"><tt>", "</tt>") else {
            continue;
        };

        let job = html_escape::decode_html_entities(job_name.trim()).into_owned();
        let platform = html_escape::decode_html_entities(platform.trim()).into_owned();

        let (attrpath, nix_attr) = if is_nixos {
            if !job.starts_with("nixos.") {
                continue;
            }
            (job.clone(), job.clone())
        } else {
            (format!("nixpkgs.{job}"), job.clone())
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

fn extract_build_links(row: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut rest = row;

    while let Some((prefix_idx, prefix_len)) = find_build_href(rest) {
        rest = &rest[prefix_idx + prefix_len..];
        let Some(end_id) = rest.find('"') else {
            break;
        };
        let Ok(id) = rest[..end_id].parse::<u64>() else {
            break;
        };

        let Some(gt) = rest[end_id..].find('>') else {
            break;
        };
        let text_start = end_id + gt + 1;
        let Some(text_end_rel) = rest[text_start..].find("</a>") else {
            break;
        };
        let text = rest[text_start..text_start + text_end_rel].to_string();
        out.push((id, text));
        rest = &rest[text_start + text_end_rel + "</a>".len()..];
    }

    out
}

fn extract_status(row: &str) -> Option<String> {
    let img = extract_between(row, "<img ", ">")?;
    if !img.contains("class=\"build-status\"") {
        return None;
    }
    extract_attr(img, "title")
}

fn find_build_href(haystack: &str) -> Option<(usize, usize)> {
    const ABSOLUTE: &str = "href=\"https://hydra.nixos.org/build/";
    const RELATIVE: &str = "href=\"/build/";

    match (haystack.find(ABSOLUTE), haystack.find(RELATIVE)) {
        (Some(abs), Some(rel)) => {
            if abs <= rel {
                Some((abs, ABSOLUTE.len()))
            } else {
                Some((rel, RELATIVE.len()))
            }
        }
        (Some(abs), None) => Some((abs, ABSOLUTE.len())),
        (None, Some(rel)) => Some((rel, RELATIVE.len())),
        (None, None) => None,
    }
}

fn extract_all<'a>(haystack: &'a str, start: &str, end: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = haystack;

    while let Some(start_idx) = rest.find(start) {
        let after_start = &rest[start_idx + start.len()..];
        let Some(end_idx) = after_start.find(end) else {
            break;
        };
        out.push(&after_start[..end_idx]);
        rest = &after_start[end_idx + end.len()..];
    }

    out
}

fn extract_between<'a>(haystack: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_idx = haystack.find(start)? + start.len();
    let rest = &haystack[start_idx..];
    let end_idx = rest.find(end)?;
    Some(&rest[..end_idx])
}

fn extract_attr(haystack: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start_idx = haystack.find(&needle)? + needle.len();
    let rest = &haystack[start_idx..];
    let end_idx = rest.find('"')?;
    Some(html_escape::decode_html_entities(&rest[..end_idx]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{parse_eval_html, BuildStatus};

    #[test]
    fn parses_failed_and_dependency_failed_rows_from_eval_html() {
        let html = r#"
        <table><tbody>
          <tr>
            <td><img src="https://hydra.nixos.org/static/images/emojione-red-x-274c.svg" height="16" width="16" title="Failed" alt="Failed" class="build-status" /></td>
            <td><a class="row-link" href="https://hydra.nixos.org/build/324447240">324447240</a></td>
            <td><a href="https://hydra.nixos.org/build/324447240">azure-cli-extensions.interactive.x86_64-linux</a></td>
            <td class="nowrap"><time title="2026-04-05 21:45:51 (UTC)">2026-04-05</time></td>
            <td>python3.13-interactive-1.0.0b1</td>
            <td class="nowrap"><tt>x86_64-linux</tt></td>
          </tr>
          <tr>
            <td><img src="https://hydra.nixos.org/static/images/emojione-gray-x-2716.svg" height="16" width="16" title="Dependency failed" alt="Dependency failed" class="build-status" /></td>
            <td><a class="row-link" href="https://hydra.nixos.org/build/324447990">324447990</a></td>
            <td><a href="https://hydra.nixos.org/build/324447990">base45.x86_64-darwin</a></td>
            <td class="nowrap"><time title="2026-04-06 11:21:52 (UTC)">2026-04-06</time></td>
            <td>base45-20230124</td>
            <td class="nowrap"><tt>x86_64-darwin</tt></td>
          </tr>
        </tbody></table>
        "#;

        let builds = parse_eval_html(html, false, 0).expect("expected parser to succeed");
        assert_eq!(builds.len(), 2);

        assert_eq!(builds[0].hydra_id, 324447240);
        assert_eq!(builds[0].attrpath, "nixpkgs.azure-cli-extensions.interactive.x86_64-linux");
        assert_eq!(builds[0].nix_attr, "azure-cli-extensions.interactive.x86_64-linux");
        assert_eq!(builds[0].platform, "x86_64-linux");
        assert!(matches!(builds[0].status, BuildStatus::Direct));

        assert_eq!(builds[1].hydra_id, 324447990);
        assert_eq!(builds[1].attrpath, "nixpkgs.base45.x86_64-darwin");
        assert_eq!(builds[1].platform, "x86_64-darwin");
        assert!(matches!(builds[1].status, BuildStatus::Indirect));
    }

    #[test]
    fn nixos_eval_keeps_only_nixos_jobs() {
        let html = r#"
        <table><tbody>
          <tr>
            <td><img src="https://hydra.nixos.org/static/images/emojione-red-x-274c.svg" title="Failed" class="build-status" /></td>
            <td><a class="row-link" href="https://hydra.nixos.org/build/1">1</a></td>
            <td><a href="https://hydra.nixos.org/build/1">nixos.tests.boot.x86_64-linux</a></td>
            <td class="nowrap"><tt>x86_64-linux</tt></td>
          </tr>
          <tr>
            <td><img src="https://hydra.nixos.org/static/images/emojione-red-x-274c.svg" title="Failed" class="build-status" /></td>
            <td><a class="row-link" href="https://hydra.nixos.org/build/2">2</a></td>
            <td><a href="https://hydra.nixos.org/build/2">hello.x86_64-linux</a></td>
            <td class="nowrap"><tt>x86_64-linux</tt></td>
          </tr>
        </tbody></table>
        "#;

        let builds = parse_eval_html(html, true, 0).expect("expected parser to succeed");
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].attrpath, "nixos.tests.boot.x86_64-linux");
    }
}

/// Retries an async operation up to `max` times with exponential backoff.
/// `base_wait` is the initial wait; each retry doubles it (30s, 60s, 120s, …).
async fn retry<F, Fut, T>(max: u32, base_wait: Duration, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, reqwest::Error>>,
{
    let mut attempts = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempts < max => {
                let wait = base_wait * 2u32.pow(attempts);
                log::warn!(
                    "Attempt {}/{max} failed: {e}. Retrying in {wait:?}…",
                    attempts + 1
                );
                tokio::time::sleep(wait).await;
                attempts += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
