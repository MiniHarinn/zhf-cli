//! RSS/Atom feed generation for newly-failing packages.
//!
//! CI has no persistent state, so we bootstrap from the previously-deployed
//! `state.json` on GitHub Pages: fetch it over HTTP, diff against current
//! failures, carry over `first_seen` timestamps for packages that are still
//! failing, set `first_seen = now` for anything new, then write an updated
//! state file plus per-channel and per-maintainer Atom feeds.

use anyhow::{Context, Result};
use atom_syndication::{
    Category, Entry, Feed, FixedDateTime, Generator, Link, Person, Text,
};
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use zhf_types::FailureItem;

/// Maximum entries kept per feed (channel or maintainer). Older currently-failing
/// items still live in `state.json` but are dropped from the rendered XML.
const FEED_RETENTION: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEntry {
    /// RFC-3339 timestamp, stable across runs while the package stays failing.
    pub first_seen: String,
    pub hydra_id: u64,
    /// "direct" or "indirect"
    pub kind: String,
    #[serde(default)]
    pub maintainers: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub schema: u32,
    pub generated_at: String,
    /// Keyed by `"{channel_slug}|{attrpath}"`.
    pub failures: BTreeMap<String, StateEntry>,
}

pub struct ChannelDisplay {
    pub project: String,
    pub jobset: String,
}

pub struct CurrentFailure<'a> {
    pub channel_slug: &'a str,
    pub item: &'a FailureItem,
    /// "direct" or "indirect"
    pub kind: &'static str,
}

pub fn state_key(channel_slug: &str, attrpath: &str) -> String {
    format!("{channel_slug}|{attrpath}")
}

/// Fetch the previously-published `state.json` so we can diff.
/// 404 or network failure ⇒ `Ok(None)` and we treat this as a first run.
pub async fn load_previous_state(client: &Client, base_url: &str) -> Result<Option<State>> {
    let url = format!("{base_url}/data/state.json");
    log::info!("Fetching previous state from {url}");

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("state.json fetch errored ({e}) — treating as first run");
            return Ok(None);
        }
    };

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        log::info!("No previous state.json (404) — first run, no entries will be emitted");
        return Ok(None);
    }

    let resp = resp.error_for_status().context("fetching state.json")?;
    let state: State = resp.json().await.context("parsing state.json")?;
    log::info!("Loaded previous state ({} entries)", state.failures.len());
    Ok(Some(state))
}

/// Build next state from current failures. `first_seen` is carried over for
/// entries that existed in `prev`; everything else gets `now`.
///
/// Returns the new state plus the keys considered "newly failing" (for logging).
/// On a first run (`prev == None`) no keys are marked new, to avoid flooding.
pub fn compute_next_state(
    prev: Option<&State>,
    current: &[CurrentFailure<'_>],
    now: DateTime<Utc>,
) -> (State, Vec<String>) {
    let now_rfc = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    let is_first_run = prev.is_none();
    let prev_failures = prev.map(|s| &s.failures);

    let mut next: BTreeMap<String, StateEntry> = BTreeMap::new();
    let mut new_keys: Vec<String> = Vec::new();

    for cur in current {
        let key = state_key(cur.channel_slug, &cur.item.attrpath);
        let prev_entry = prev_failures.and_then(|m| m.get(&key));
        let first_seen = prev_entry
            .map(|e| e.first_seen.clone())
            .unwrap_or_else(|| now_rfc.clone());

        if prev_entry.is_none() && !is_first_run {
            new_keys.push(key.clone());
        }

        next.insert(
            key,
            StateEntry {
                first_seen,
                hydra_id: cur.item.hydra_id,
                kind: cur.kind.to_string(),
                maintainers: cur.item.maintainers.clone(),
            },
        );
    }

    let state = State {
        schema: 1,
        generated_at: now_rfc,
        failures: next,
    };
    (state, new_keys)
}

pub fn write_state(output_dir: &Path, state: &State) -> Result<()> {
    let path = output_dir.join("data").join("state.json");
    let json = serde_json::to_string(state)?;
    fs::write(&path, json).with_context(|| format!("writing {path:?}"))?;
    Ok(())
}

/// Render per-channel and per-maintainer Atom feeds from the current state.
///
/// Each feed lists the 200 most-recent currently-failing items for that scope,
/// sorted by `first_seen` desc. Entry IDs include the `first_seen` date so
/// that a fix-then-rebreak produces a new entry (and thus a re-notification).
pub fn write_feeds(
    output_dir: &Path,
    state: &State,
    base_url: &str,
    channel_lookup: &HashMap<String, ChannelDisplay>,
    now: DateTime<Utc>,
) -> Result<()> {
    let mut by_channel: HashMap<String, Vec<(String, &StateEntry)>> = HashMap::new();
    let mut by_maintainer: HashMap<String, Vec<(String, &StateEntry)>> = HashMap::new();

    for (key, entry) in &state.failures {
        let Some((channel, _attrpath)) = key.split_once('|') else { continue };
        by_channel
            .entry(channel.to_string())
            .or_default()
            .push((key.clone(), entry));
        for m in &entry.maintainers {
            by_maintainer
                .entry(m.clone())
                .or_default()
                .push((key.clone(), entry));
        }
    }

    let feed_dir = output_dir.join("feed");
    fs::create_dir_all(&feed_dir)?;
    let maint_dir = feed_dir.join("maintainer");
    fs::create_dir_all(&maint_dir)?;

    for (channel, mut entries) in by_channel {
        sort_and_truncate(&mut entries);
        let disp = channel_lookup.get(&channel);
        let title = match disp {
            Some(d) => format!("Hydra failures — {}/{}", d.project, d.jobset),
            None => format!("Hydra failures — {channel}"),
        };
        let self_url = format!("{base_url}/feed/{channel}.xml");
        let feed_id = format!("urn:zhf:feed:channel:{channel}");
        let xml = render_feed(&feed_id, &title, &self_url, now, &entries, channel_lookup);
        fs::write(feed_dir.join(format!("{channel}.xml")), xml)?;
    }

    log::info!(
        "Wrote per-maintainer feeds for {} maintainers",
        by_maintainer.len()
    );

    for (handle, mut entries) in by_maintainer {
        sort_and_truncate(&mut entries);
        let title = format!("Hydra failures for @{handle}");
        let sanitized = sanitize_handle(&handle);
        let self_url = format!("{base_url}/feed/maintainer/{sanitized}.xml");
        let feed_id = format!("urn:zhf:feed:maintainer:{sanitized}");
        let xml = render_feed(&feed_id, &title, &self_url, now, &entries, channel_lookup);
        fs::write(maint_dir.join(format!("{sanitized}.xml")), xml)?;
    }

    Ok(())
}

fn sort_and_truncate(entries: &mut Vec<(String, &StateEntry)>) {
    entries.sort_by(|a, b| b.1.first_seen.cmp(&a.1.first_seen));
    entries.truncate(FEED_RETENTION);
}

/// Conservatively sanitize a GitHub handle for use as a filename. GitHub
/// handles are already ASCII alphanumeric + dashes, but defensively strip
/// anything unexpected in case maintainer data contains weird strings.
fn sanitize_handle(handle: &str) -> String {
    handle
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn render_feed(
    feed_id: &str,
    title: &str,
    self_url: &str,
    updated: DateTime<Utc>,
    entries: &[(String, &StateEntry)],
    channel_lookup: &HashMap<String, ChannelDisplay>,
) -> String {
    let mut feed = Feed::default();
    feed.set_id(feed_id);
    feed.set_title(title);
    feed.set_updated(updated.fixed_offset());

    feed.set_links(vec![Link {
        href: self_url.to_string(),
        rel: "self".to_string(),
        ..Default::default()
    }]);

    feed.set_generator(Some(Generator {
        value: "zhf-generator".to_string(),
        uri: Some("https://github.com/moment/zhf".to_string()),
        version: None,
    }));

    feed.set_authors(vec![Person {
        name: "zhf-generator".to_string(),
        email: None,
        uri: None,
    }]);

    let atom_entries: Vec<Entry> = entries
        .iter()
        .map(|(key, se)| make_entry(key, se, channel_lookup))
        .collect();
    feed.set_entries(atom_entries);

    feed.to_string()
}

fn make_entry(
    key: &str,
    se: &StateEntry,
    channel_lookup: &HashMap<String, ChannelDisplay>,
) -> Entry {
    let (channel_slug, attrpath) = key.split_once('|').unwrap_or(("", key));
    let display_channel = channel_lookup
        .get(channel_slug)
        .map(|d| format!("{}/{}", d.project, d.jobset))
        .unwrap_or_else(|| channel_slug.to_string());

    let first_seen_fixed = parse_rfc3339_or_epoch(&se.first_seen);
    // First-seen date in the id guarantees a fresh GUID when a package
    // transitions fix → re-break, triggering a new reader notification.
    let first_seen_date = first_seen_fixed.format("%Y-%m-%d");

    let kind_suffix = if se.kind == "indirect" { " (indirect)" } else { "" };
    let title = format!("{attrpath} started failing in {display_channel}{kind_suffix}");

    let maintainers_str = if se.maintainers.is_empty() {
        "(no maintainers)".to_string()
    } else {
        se.maintainers
            .iter()
            .map(|m| format!("@{m}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let summary = format!(
        "{attrpath} ({kind}) in {display_channel}. Maintainers: {maintainers_str}. Build: https://hydra.nixos.org/build/{hydra_id}",
        kind = se.kind,
        hydra_id = se.hydra_id,
    );

    let mut categories = vec![
        Category { term: channel_slug.to_string(), scheme: None, label: None },
        Category { term: se.kind.clone(), scheme: None, label: None },
    ];
    for m in &se.maintainers {
        categories.push(Category {
            term: m.clone(),
            scheme: Some("maintainer".to_string()),
            label: None,
        });
    }

    let mut entry = Entry::default();
    entry.set_id(format!("urn:zhf:{channel_slug}:{attrpath}:{first_seen_date}"));
    entry.set_title(title);
    entry.set_updated(first_seen_fixed);
    entry.set_published(Some(first_seen_fixed));
    entry.set_links(vec![Link {
        href: format!("https://hydra.nixos.org/build/{}", se.hydra_id),
        rel: "alternate".to_string(),
        ..Default::default()
    }]);
    entry.set_summary(Some(Text::plain(summary)));
    entry.set_categories(categories);
    entry
}

fn parse_rfc3339_or_epoch(s: &str) -> FixedDateTime {
    DateTime::parse_from_rfc3339(s)
        .unwrap_or_else(|_| DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z").unwrap())
}
