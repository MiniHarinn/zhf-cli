use anyhow::Result;
use tabled::{
    Table, Tabled,
    settings::{Style, object::Rows, Alignment, Modify},
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
    let rows = vec![
        StatsRow { field: "Target".into(), value: s.target.clone() },
        StatsRow { field: "Last Check".into(), value: s.last_check.clone() },
        StatsRow {
            field: "Latest Linux Evaluation".into(),
            value: format!("#{} on {}", s.linux_eval, s.linux_eval_time),
        },
        StatsRow {
            field: "Latest Darwin Evaluation".into(),
            value: format!("#{} on {}", s.darwin_eval, s.darwin_eval_time),
        },
        StatsRow { field: "Failing on aarch64-darwin".into(), value: s.aarch64_darwin.to_string() },
        StatsRow { field: "Failing on aarch64-linux".into(), value: s.aarch64_linux.to_string() },
        StatsRow { field: "Failing on x86_64-darwin".into(), value: s.x86_64_darwin.to_string() },
        StatsRow { field: "Failing on x86_64-linux".into(), value: s.x86_64_linux.to_string() },
        StatsRow { field: "Total Failed Builds".into(), value: s.total.to_string() },
    ];

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
}

pub fn print_problematic(items: &[FailureItem], meta: &PageMeta) {
    if items.is_empty() {
        println!("No problematic dependencies found.");
        return;
    }

    let rows: Vec<ProblematicRow> = items
        .iter()
        .map(|i| ProblematicRow {
            attrpath: i.attrpath.clone(),
            platform: i.platform.clone(),
            dependants: i.dependants.map(|d| d.to_string()).unwrap_or_else(|| "-".into()),
            hydra_url: i.hydra_url.clone(),
        })
        .collect();

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
    println!("  Last updated: {}", meta.last_check);
    println!("  Total: {} entries", items.len());
}

pub fn print_failures(items: &[FailureItem], meta: &PageMeta) {
    if items.is_empty() {
        println!("No failures found matching the given filters.");
        return;
    }

    let rows: Vec<FailureRow> = items
        .iter()
        .map(|i| FailureRow {
            attrpath: i.attrpath.clone(),
            platform: i.platform.clone(),
            maintainer: i.maintainer.clone().unwrap_or_else(|| "-".into()),
            hydra_url: i.hydra_url.clone(),
            kind: i.kind.into(),
        })
        .collect();

    let table = Table::new(rows)
        .with(Style::rounded())
        .with(Modify::new(Rows::first()).with(Alignment::center()))
        .to_string();

    println!("{table}");
    println!("  Last updated: {}", meta.last_check);
    println!("  Total: {} entries", items.len());
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
    println!("Exported {} rows to {dest}", items.len());
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
    println!("Exported {} rows to {dest}", items.len());
    Ok(())
}
