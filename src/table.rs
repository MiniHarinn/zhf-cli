use anyhow::Result;
use owo_colors::{OwoColorize, Stream::Stdout, Style};
use tabled::{
    settings::{object::Rows, Alignment, Modify, Style as TableStyle},
    Table, Tabled,
};
use zhf_types::IndexJson;

use crate::fetcher::FailureEntry;

#[derive(Tabled)]
struct StatsRow {
    #[tabled(rename = "Field")]
    field: String,
    #[tabled(rename = "Value")]
    value: String,
}

#[derive(Tabled)]
struct FailureRow {
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
    let nixos_eval = format!("#{}", s.nixos_eval.id);
    let nixpkgs_eval = format!("#{}", s.nixpkgs_eval.id);

    let mut rows = vec![
        StatsRow {
            field: label("Generated At"),
            value: style_text(&s.generated_at, Style::new().bright_black()),
        },
        StatsRow {
            field: label("nixos/unstable Eval"),
            value: format!(
                "{} {} {}",
                style_text(&nixos_eval, Style::new().green().bold()),
                style_text("on", Style::new().bright_black()),
                style_text(&s.nixos_eval.time, Style::new().green())
            ),
        },
        StatsRow {
            field: label("nixpkgs/unstable Eval"),
            value: format!(
                "{} {} {}",
                style_text(&nixpkgs_eval, Style::new().blue().bold()),
                style_text("on", Style::new().bright_black()),
                style_text(&s.nixpkgs_eval.time, Style::new().blue())
            ),
        },
    ];

    let c = &s.counts;
    let platform_counts = [
        ("aarch64-darwin", c.aarch64_darwin),
        ("aarch64-linux", c.aarch64_linux),
        ("x86_64-darwin", c.x86_64_darwin),
        ("x86_64-linux", c.x86_64_linux),
        ("i686-linux", c.i686_linux),
    ];
    for (plat, n) in platform_counts {
        if n > 0 {
            rows.push(StatsRow {
                field: label(&format!("Failing on {plat}")),
                value: count(n),
            });
        }
    }
    rows.push(StatsRow {
        field: label("Total Failed Builds"),
        value: count(c.total),
    });

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
                &format!("#{}", e.item.hydra_id),
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
    wtr.write_record(["Attrpath", "Platform", "Maintainers", "Hydra Build", "Kind"])?;
    for e in entries {
        let maintainers = e.item.maintainers.join(",");
        wtr.write_record([
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
