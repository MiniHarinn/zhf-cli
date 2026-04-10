use anyhow::{Context, Result};
use scraper::{Html, Selector};

use crate::cli::JobFilter;

const BASE_URL: &str = "https://zh.fail";

pub struct Stats {
    pub target: String,
    pub last_check: String,
    pub linux_eval: String,
    pub linux_eval_time: String,
    pub darwin_eval: String,
    pub darwin_eval_time: String,
    pub aarch64_darwin: u32,
    pub aarch64_linux: u32,
    pub x86_64_darwin: u32,
    pub x86_64_linux: u32,
    pub total: u32,
}

pub struct FailureItem {
    pub attrpath: String,
    pub platform: String,
    pub maintainer: Option<String>,
    pub hydra_url: String,
    pub kind: &'static str, // "direct", "indirect", or "problematic"
    pub dependants: Option<u32>, // only for "problematic"
}

pub struct PageMeta {
    pub last_check: String,
}

fn fetch_html(url: &str) -> Result<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("zhf-cli/0.1 (github scraper)")
        .build()?;
    let body = client
        .get(url)
        .send()
        .with_context(|| format!("fetching {url}"))?
        .text()?;
    Ok(body)
}

pub fn fetch_stats() -> Result<Stats> {
    let html = fetch_html(&format!("{BASE_URL}/index.html"))?;
    let doc = Html::parse_document(&html);
    let td_sel = Selector::parse("table.compact tr td").unwrap();

    let cells: Vec<String> = doc
        .select(&td_sel)
        .map(|el| el.text().collect::<String>().trim().to_string())
        .collect();

    // Parse the compact table: alternating label/value pairs
    let mut target = String::new();
    let mut last_check = String::new();
    let mut linux_eval = String::new();
    let mut linux_eval_time = String::new();
    let mut darwin_eval = String::new();
    let mut darwin_eval_time = String::new();
    let mut aarch64_darwin = 0u32;
    let mut aarch64_linux = 0u32;
    let mut x86_64_darwin = 0u32;
    let mut x86_64_linux = 0u32;
    let mut total = 0u32;

    // Also grab links for eval IDs
    let a_sel = Selector::parse("table.compact tr td a").unwrap();
    let links: Vec<(String, String)> = doc
        .select(&a_sel)
        .map(|el| {
            let text = el.text().collect::<String>().trim().to_string();
            let href = el.value().attr("href").unwrap_or("").to_string();
            (text, href)
        })
        .collect();

    let mut eval_links: Vec<(String, String)> = links
        .into_iter()
        .filter(|(_, href)| href.contains("hydra.nixos.org/eval/"))
        .collect();

    for chunk in cells.chunks(2) {
        if chunk.len() < 2 {
            continue;
        }
        let label = &chunk[0];
        let value = &chunk[1];

        if label.contains("Current target") {
            target = value.clone();
        } else if label.contains("Last check") {
            // value might be "2026-04-10 16:13:44 (UTC) (Triggered by schedule)"
            last_check = value.split('(').next().unwrap_or(value).trim().to_string();
            // Keep just the datetime and UTC
            if let Some(idx) = value.find("(UTC)") {
                last_check = value[..idx + 5].trim().to_string();
            }
        } else if label.contains("Latest Linux evaluation") {
            if let Some((id, href)) = eval_links.first() {
                linux_eval = id.clone();
                let _ = href;
            }
            // parse "on DATETIME (UTC)" from value
            if let Some(pos) = value.find(" on ") {
                let after = &value[pos + 4..];
                if let Some(idx) = after.find("(UTC)") {
                    linux_eval_time = after[..idx + 5].trim().to_string();
                }
            }
            if !eval_links.is_empty() {
                eval_links.remove(0);
            }
        } else if label.contains("Latest Darwin evaluation") {
            if let Some((id, _)) = eval_links.first() {
                darwin_eval = id.clone();
            }
            if let Some(pos) = value.find(" on ") {
                let after = &value[pos + 4..];
                if let Some(idx) = after.find("(UTC)") {
                    darwin_eval_time = after[..idx + 5].trim().to_string();
                }
            }
            if !eval_links.is_empty() {
                eval_links.remove(0);
            }
        } else if label.contains("aarch64-darwin") {
            aarch64_darwin = value.parse().unwrap_or(0);
        } else if label.contains("aarch64-linux") {
            aarch64_linux = value.parse().unwrap_or(0);
        } else if label.contains("x86_64-darwin") {
            x86_64_darwin = value.parse().unwrap_or(0);
        } else if label.contains("x86_64-linux") {
            x86_64_linux = value.parse().unwrap_or(0);
        } else if label.contains("Total failed") {
            total = value.parse().unwrap_or(0);
        }
    }

    Ok(Stats {
        target,
        last_check,
        linux_eval,
        linux_eval_time,
        darwin_eval,
        darwin_eval_time,
        aarch64_darwin,
        aarch64_linux,
        x86_64_darwin,
        x86_64_linux,
        total,
    })
}

pub fn fetch_problematic() -> Result<(Vec<FailureItem>, PageMeta)> {
    let html = fetch_html(&format!("{BASE_URL}/index.html"))?;
    let doc = Html::parse_document(&html);

    let meta = extract_meta(&doc);

    // The problematic table has columns: Job, Platform, Number of dependants
    // Job column contains <details><summary><a href=...>name</a></summary>...</details>
    let table_sel = Selector::parse("table:not(.compact)").unwrap();
    let tr_sel = Selector::parse("tr").unwrap();
    let td_sel = Selector::parse("td").unwrap();
    // Select the first <a> inside <summary> inside the first <td>
    let summary_a_sel = Selector::parse("summary > a").unwrap();

    let mut items = Vec::new();

    if let Some(table) = doc.select(&table_sel).next() {
        let rows: Vec<_> = table.select(&tr_sel).collect();
        for row in rows.iter().skip(1) {
            let cols: Vec<_> = row.select(&td_sel).collect();
            if cols.len() < 3 {
                continue;
            }

            // Col 0: Job (link inside summary)
            let first_col = &cols[0];
            let link = first_col.select(&summary_a_sel).next();
            let (attrpath, hydra_url) = if let Some(a) = link {
                (
                    a.text().collect::<String>().trim().to_string(),
                    a.value().attr("href").unwrap_or("").to_string(),
                )
            } else {
                continue;
            };

            if attrpath.is_empty() {
                continue;
            }

            // Col 1: Platform
            let platform = cols[1].text().collect::<String>().trim().to_string();

            // Col 2: Number of dependants
            let dependants: Option<u32> = cols[2]
                .text()
                .collect::<String>()
                .trim()
                .parse()
                .ok();

            items.push(FailureItem {
                attrpath,
                platform,
                maintainer: None,
                hydra_url,
                kind: "problematic",
                dependants,
            });
        }
    }

    Ok((items, meta))
}

pub fn fetch_failures(job_filter: JobFilter) -> Result<(Vec<FailureItem>, PageMeta)> {
    let html = fetch_html(&format!("{BASE_URL}/failed/all.html"))?;
    let doc = Html::parse_document(&html);

    // Extract meta from parent page for last_check
    let index_html = fetch_html(&format!("{BASE_URL}/index.html"))?;
    let index_doc = Html::parse_document(&index_html);
    let meta = extract_meta(&index_doc);

    let mut items = Vec::new();

    // The page has two h2 sections: #direct and #indirect
    // We parse both tables
    let h2_sel = Selector::parse("h2").unwrap();
    let table_sel = Selector::parse("table").unwrap();
    let tr_sel = Selector::parse("tr").unwrap();
    let td_sel = Selector::parse("td").unwrap();
    let a_sel = Selector::parse("a").unwrap();

    let tables: Vec<_> = doc.select(&table_sel).collect();
    let h2s: Vec<_> = doc.select(&h2_sel).collect();

    // Map h2 id to table index
    // Typically: h2#direct -> tables[0], h2#indirect -> tables[1]
    let section_ids: Vec<&str> = h2s
        .iter()
        .map(|h| h.value().id().unwrap_or(""))
        .collect();

    for (i, table) in tables.iter().enumerate() {
        let kind: &'static str = if section_ids.get(i).copied() == Some("indirect") {
            "indirect"
        } else {
            "direct"
        };

        let include = match job_filter {
            JobFilter::All => true,
            JobFilter::Direct => kind == "direct",
            JobFilter::Indirect => kind == "indirect",
        };
        if !include {
            continue;
        }

        let rows: Vec<_> = table.select(&tr_sel).collect();
        for row in rows.iter().skip(1) {
            let cols: Vec<_> = row.select(&td_sel).collect();
            if cols.is_empty() {
                continue;
            }

            let first_col = cols[0].clone();
            let link = first_col.select(&a_sel).next();
            let (attrpath, hydra_url) = if let Some(a) = link {
                (
                    a.text().collect::<String>().trim().to_string(),
                    a.value().attr("href").unwrap_or("").to_string(),
                )
            } else {
                continue;
            };

            if attrpath.is_empty() {
                continue;
            }

            let platform = if cols.len() >= 3 {
                cols[2].text().collect::<String>().trim().to_string()
            } else {
                extract_platform(&attrpath)
            };

            // Maintainer is col index 3 for direct (has Maintainer column)
            // indirect table has no maintainer column
            let maintainer = if kind == "direct" && cols.len() >= 4 {
                let m = cols[3].text().collect::<String>().trim().to_string();
                if m.is_empty() { None } else { Some(m) }
            } else {
                None
            };

            items.push(FailureItem {
                attrpath,
                platform,
                maintainer,
                hydra_url,
                kind,
                dependants: None,
            });
        }
    }

    Ok((items, meta))
}

fn extract_meta(doc: &Html) -> PageMeta {
    let td_sel = Selector::parse("table.compact tr td").unwrap();
    let cells: Vec<String> = doc
        .select(&td_sel)
        .map(|el| el.text().collect::<String>().trim().to_string())
        .collect();

    let mut last_check = String::from("unknown");
    for chunk in cells.chunks(2) {
        if chunk.len() >= 2 && chunk[0].contains("Last check") {
            let val = &chunk[1];
            if let Some(idx) = val.find("(UTC)") {
                last_check = val[..idx + 5].trim().to_string();
            } else {
                last_check = val.clone();
            }
            break;
        }
    }

    PageMeta { last_check }
}

fn extract_platform(attrpath: &str) -> String {
    // Last segment after final dot is often the platform
    let parts: Vec<&str> = attrpath.split('.').collect();
    if let Some(last) = parts.last() {
        let known = ["aarch64-linux", "x86_64-linux", "aarch64-darwin", "x86_64-darwin"];
        if known.contains(last) {
            return last.to_string();
        }
    }
    String::from("unknown")
}
