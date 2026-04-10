use anyhow::Result;
use owo_colors::{OwoColorize, Stream::Stdout, Style};
use tabled::{
    settings::{object::Rows, Alignment, Modify, Style as TableStyle},
    Table, Tabled,
};

use crate::scraper::{FailureItem, PageMeta, Stats};

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
    #[tabled(rename = "Maintainer")]
    maintainer: String,
    #[tabled(rename = "Hydra Build")]
    hydra_url: String,
    #[tabled(rename = "Kind")]
    kind: String,
}

#[derive(Tabled)]
struct ProblematicRow {
    #[tabled(rename = "Job")]
    attrpath: String,
    #[tabled(rename = "Platform")]
    platform: String,
    #[tabled(rename = "Dependants")]
    dependants: String,
    #[tabled(rename = "Hydra Build")]
    hydra_url: String,
}

pub fn print_stats(s: &Stats) {
    let linux_eval = format!("#{}", s.linux_eval);
    let darwin_eval = format!("#{}", s.darwin_eval);

    let rows = vec![
        StatsRow {
            field: label("Target"),
            value: style_text(&s.target, Style::new().cyan()),
        },
        StatsRow {
            field: label("Last Check"),
            value: style_text(&s.last_check, Style::new().bright_black()),
        },
        StatsRow {
            field: label("Latest Linux Evaluation"),
            value: format!(
                "{} {} {}",
                style_text(&linux_eval, Style::new().green().bold()),
                style_text("on", Style::new().bright_black()),
                style_text(&s.linux_eval_time, Style::new().green())
            ),
        },
        StatsRow {
            field: label("Latest Darwin Evaluation"),
            value: format!(
                "{} {} {}",
                style_text(&darwin_eval, Style::new().blue().bold()),
                style_text("on", Style::new().bright_black()),
                style_text(&s.darwin_eval_time, Style::new().blue())
            ),
        },
        StatsRow {
            field: label("Failing on aarch64-darwin"),
            value: count(s.aarch64_darwin),
        },
        StatsRow {
            field: label("Failing on aarch64-linux"),
            value: count(s.aarch64_linux),
        },
        StatsRow {
            field: label("Failing on x86_64-darwin"),
            value: count(s.x86_64_darwin),
        },
        StatsRow {
            field: label("Failing on x86_64-linux"),
            value: count(s.x86_64_linux),
        },
        StatsRow {
            field: label("Total Failed Builds"),
            value: count(s.total),
        },
    ];

    let table = Table::new(rows)
        .with(TableStyle::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
}

pub fn print_problematic(items: &[FailureItem], meta: &PageMeta) {
    if items.is_empty() {
        println!(
            "{}",
            style_text(
                "No problematic dependencies found.",
                Style::new().green().bold()
            )
        );
        return;
    }

    let rows: Vec<ProblematicRow> = items
        .iter()
        .map(|i| ProblematicRow {
            attrpath: style_text(&i.attrpath, Style::new().cyan()),
            platform: platform(&i.platform),
            dependants: i.dependants.map(count).unwrap_or_else(|| "-".into()),
            hydra_url: style_text(&i.hydra_url, Style::new().underline().blue()),
        })
        .collect();

    let table = Table::new(rows)
        .with(TableStyle::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
    print_footer(meta, items.len());
}

pub fn print_failures(items: &[FailureItem], meta: &PageMeta) {
    if items.is_empty() {
        println!(
            "{}",
            style_text(
                "No failures found matching the given filters.",
                Style::new().green().bold()
            )
        );
        return;
    }

    let rows: Vec<FailureRow> = items
        .iter()
        .map(|i| FailureRow {
            attrpath: style_text(&i.attrpath, Style::new().cyan()),
            platform: platform(&i.platform),
            maintainer: i
                .maintainer
                .as_deref()
                .map(|name| style_text(name, Style::new().magenta()))
                .unwrap_or_else(|| style_text("-", Style::new().bright_black())),
            hydra_url: style_text(&i.hydra_url, Style::new().underline().blue()),
            kind: kind(i.kind),
        })
        .collect();

    let table = Table::new(rows)
        .with(TableStyle::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
    print_footer(meta, items.len());
}

pub fn export_csv_problematic(items: &[FailureItem], dest: &str) -> Result<()> {
    let mut wtr = csv::Writer::from_path(dest)?;
    wtr.write_record(["Job", "Platform", "Dependants", "Hydra Build"])?;
    for item in items {
        wtr.write_record([
            &item.attrpath,
            &item.platform,
            &item.dependants.map(|d| d.to_string()).unwrap_or_default(),
            &item.hydra_url,
        ])?;
    }
    wtr.flush()?;
    println!(
        "{} {} {}",
        style_text("Exported", Style::new().green().bold()),
        count(items.len() as u32),
        style_text(&format!("rows to {dest}"), Style::new().cyan())
    );
    Ok(())
}

pub fn export_csv(items: &[FailureItem], dest: &str) -> Result<()> {
    let mut wtr = csv::Writer::from_path(dest)?;
    wtr.write_record(["Attrpath", "Platform", "Maintainer", "Hydra Build", "Kind"])?;
    for item in items {
        wtr.write_record([
            &item.attrpath,
            &item.platform,
            item.maintainer.as_deref().unwrap_or(""),
            &item.hydra_url,
            item.kind,
        ])?;
    }
    wtr.flush()?;
    println!(
        "{} {} {}",
        style_text("Exported", Style::new().green().bold()),
        count(items.len() as u32),
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
        "aarch64-linux" | "x86_64-linux" => Style::new().green(),
        "aarch64-darwin" | "x86_64-darwin" => Style::new().blue(),
        _ => Style::new().yellow(),
    };
    style_text(value, style)
}

fn kind(value: &str) -> String {
    let style = match value {
        "direct" => Style::new().red().bold(),
        "indirect" => Style::new().yellow().bold(),
        "problematic" => Style::new().magenta().bold(),
        _ => Style::new().bold(),
    };
    style_text(value, style)
}

fn print_footer(meta: &PageMeta, total: usize) {
    println!(
        "  {} {}",
        style_text("Last updated:", Style::new().bold()),
        style_text(&meta.last_check, Style::new().bright_black())
    );
    println!(
        "  {} {} {}",
        style_text("Total:", Style::new().bold()),
        count(total as u32),
        style_text("entries", Style::new().bright_black())
    );
}

fn style_text(text: &str, style: Style) -> String {
    text.if_supports_color(Stdout, |value| value.style(style))
        .to_string()
}
