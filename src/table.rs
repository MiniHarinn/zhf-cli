use anyhow::Result;
use owo_colors::{OwoColorize, Stream::Stdout, Style};
use tabled::{
    settings::{object::Rows, Alignment, Modify, Style as TableStyle},
    Table, Tabled,
};
use zhf_types::IndexJson;

use crate::fetcher::FailureEntry;

// Display order and human-readable labels for channels
const CHANNEL_ORDER: &[(&str, &str)] = &[
    ("nixos_unstable",       "nixos/unstable"),
    ("nixos_staging",        "nixos/staging"),
    ("nixpkgs_unstable",     "nixpkgs/unstable"),
    ("nixpkgs_staging_next", "nixpkgs/staging-next"),
];

#[derive(Tabled)]
struct StatsRow {
    #[tabled(rename = "Field")]
    field: String,
    #[tabled(rename = "Value")]
    value: String,
}

#[derive(Tabled)]
struct FailureRow {
    #[tabled(rename = "Channel")]
    channel: String,
    #[tabled(rename = "Attrpath")]
    attrpath: String,
    #[tabled(rename = "Platform")]
    platform: String,
    #[tabled(rename = "Maintainers")]
    maintainers: String,
    #[tabled(rename = "Hydra Build")]
    hydra_url: String,
    #[tabled(rename = "Kind")]
    kind: String,
}

pub fn print_stats(s: &IndexJson) {
    let mut rows = vec![StatsRow {
        field: label("Generated At"),
        value: style_text(&s.generated_at, Style::new().bright_black()),
    }];

    rows.push(StatsRow { field: String::new(), value: String::new() });

    for (slug, display_name) in CHANNEL_ORDER {
        let Some(ch) = s.channels.get(*slug) else { continue };

        // Blank separator between channels (except before the first)
        if rows.len() > 2 {
            rows.push(StatsRow { field: String::new(), value: String::new() });
        }

        let eval_id = format!("#{}", ch.eval.id);
        rows.push(StatsRow {
            field: label(&format!("{display_name} Eval")),
            value: format!(
                "{} {} {}",
                style_text(&eval_id, Style::new().green().bold()),
                style_text("on", Style::new().bright_black()),
                style_text(&ch.eval.time, Style::new().green())
            ),
        });

        let platform_counts = [
            ("aarch64-darwin", ch.direct_counts.aarch64_darwin, ch.indirect_counts.aarch64_darwin),
            ("aarch64-linux",  ch.direct_counts.aarch64_linux,  ch.indirect_counts.aarch64_linux),
            ("x86_64-darwin",  ch.direct_counts.x86_64_darwin,  ch.indirect_counts.x86_64_darwin),
            ("x86_64-linux",   ch.direct_counts.x86_64_linux,   ch.indirect_counts.x86_64_linux),
            ("i686-linux",     ch.direct_counts.i686_linux,      ch.indirect_counts.i686_linux),
        ];
        for (plat, direct_n, indirect_n) in platform_counts {
            if direct_n > 0 || indirect_n > 0 {
                rows.push(StatsRow {
                    field: label(&format!("Failing on {plat}")),
                    value: format!(
                        "{} direct  {} indirect",
                        count(direct_n),
                        count(indirect_n)
                    ),
                });
            }
        }
        rows.push(StatsRow {
            field: label("Total Direct"),
            value: count(ch.direct_counts.total),
        });
        rows.push(StatsRow {
            field: label("Total Indirect"),
            value: count(ch.indirect_counts.total),
        });
    }

    let table = Table::new(rows)
        .with(TableStyle::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
}

pub fn print_failures(entries: &[FailureEntry]) {
    if entries.is_empty() {
        println!(
            "{}",
            style_text(
                "No failures found matching the given filters.",
                Style::new().green().bold()
            )
        );
        return;
    }

    let rows: Vec<FailureRow> = entries
        .iter()
        .map(|e| FailureRow {
            channel: style_text(&e.channel, Style::new().bright_black()),
            attrpath: style_text(&e.item.attrpath, Style::new().cyan()),
            platform: platform(&e.item.platform),
            maintainers: if e.item.maintainers.is_empty() {
                style_text("-", Style::new().bright_black())
            } else {
                let m = &e.item.maintainers;
                let shown = m.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                if m.len() > 2 {
                    format!(
                        "{} {}",
                        style_text(&shown, Style::new().magenta()),
                        style_text(&format!("and {} more", m.len() - 2), Style::new().bright_black()),
                    )
                } else {
                    style_text(&shown, Style::new().magenta())
                }
            },
            hydra_url: style_text(
                &format!("https://hydra.nixos.org/build/{}", e.item.hydra_id),
                Style::new().blue(),
            ),
            kind: kind(e.kind),
        })
        .collect();

    let table = Table::new(rows)
        .with(TableStyle::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
    println!(
        "  {} {}",
        style_text("Total:", Style::new().bold()),
        count(entries.len() as u32)
    );
}

pub fn export_csv(entries: &[FailureEntry], dest: &str) -> Result<()> {
    let mut wtr = csv::Writer::from_path(dest)?;
    wtr.write_record(["Channel", "Attrpath", "Platform", "Maintainers", "Hydra Build", "Kind"])?;
    for e in entries {
        let maintainers = e.item.maintainers.join(",");
        wtr.write_record([
            e.channel.as_str(),
            e.item.attrpath.as_str(),
            e.item.platform.as_str(),
            maintainers.as_str(),
            &format!("https://hydra.nixos.org/build/{}", e.item.hydra_id),
            e.kind,
        ])?;
    }
    wtr.flush()?;
    println!(
        "{} {} {}",
        style_text("Exported", Style::new().green().bold()),
        count(entries.len() as u32),
        style_text(&format!("rows to {dest}"), Style::new().cyan())
    );
    Ok(())
}

fn label(text: &str) -> String {
    style_text(text, Style::new().bold())
}

fn count(value: u32) -> String {
    let style = if value == 0 {
        Style::new().green().bold()
    } else if value < 10 {
        Style::new().yellow().bold()
    } else {
        Style::new().red().bold()
    };
    style_text(&value.to_string(), style)
}

fn platform(value: &str) -> String {
    let style = match value {
        "aarch64-linux" | "x86_64-linux" | "i686-linux" => Style::new().green(),
        "aarch64-darwin" | "x86_64-darwin" => Style::new().blue(),
        _ => Style::new().yellow(),
    };
    style_text(value, style)
}

fn kind(value: &str) -> String {
    let style = match value {
        "direct" => Style::new().red().bold(),
        "indirect" => Style::new().yellow().bold(),
        _ => Style::new().bold(),
    };
    style_text(value, style)
}

fn style_text(text: &str, style: Style) -> String {
    text.if_supports_color(Stdout, |value| value.style(style))
        .to_string()
}
