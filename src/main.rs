mod cli;
mod scraper;
mod table;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, FailsOn, JobFilter};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Stats => {
            let stats = scraper::fetch_stats()?;
            table::print_stats(&stats);
        }
        Command::Problematic { export } => {
            let (items, meta) = scraper::fetch_problematic()?;
            if let Some(dest) = export {
                table::export_csv_problematic(&items, &dest)?;
            } else {
                table::print_problematic(&items, &meta);
            }
        }
        Command::All { filter } => {
            let job_filter = cli::JobFilter::All;
            run_failures(job_filter, filter)?;
        }
        Command::Direct { filter } => {
            run_failures(JobFilter::Direct, filter)?;
        }
        Command::Indirect { filter } => {
            run_failures(JobFilter::Indirect, filter)?;
        }
    }
    Ok(())
}

fn run_failures(job_filter: JobFilter, filter: cli::FailureFilter) -> Result<()> {
    let (mut items, meta) = scraper::fetch_failures(job_filter)?;

    // Apply attrset filter
    if filter.nixpkgs {
        items.retain(|i| i.attrpath.starts_with("nixpkgs."));
    } else if filter.nixos {
        items.retain(|i| i.attrpath.starts_with("nixos."));
    }

    // Apply platform filter
    match filter.fails_on {
        FailsOn::All => {}
        FailsOn::Linux => items.retain(|i| i.platform.contains("linux")),
        FailsOn::Darwin => items.retain(|i| i.platform.contains("darwin")),
        FailsOn::Aarch64Linux => items.retain(|i| i.platform == "aarch64-linux"),
        FailsOn::X8664Linux => items.retain(|i| i.platform == "x86_64-linux"),
        FailsOn::Aarch64Darwin => items.retain(|i| i.platform == "aarch64-darwin"),
        FailsOn::X8664Darwin => items.retain(|i| i.platform == "x86_64-darwin"),
    }

    // Apply maintainer filter
    if let Some(ref name) = filter.maintainer {
        items.retain(|i| i.maintainer.as_deref() == Some(name.as_str()));
    } else if filter.no_maintainer {
        items.retain(|i| i.maintainer.is_none());
    }

    if let Some(dest) = filter.export {
        table::export_csv(&items, &dest)?;
    } else {
        table::print_failures(&items, &meta);
    }
    Ok(())
}
