mod cli;
mod fetcher;
mod table;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, FailsOn, JobFilter};
fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Stats => {
            let stats = fetcher::fetch_stats()?;
            table::print_stats(&stats);
        }
        Command::All { filter } => {
            run_failures(JobFilter::All, filter)?;
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
    let mut entries = fetcher::fetch_failures(job_filter, &filter)?;

    // Apply platform filter
    match filter.fails_on {
        FailsOn::All => {}
        FailsOn::Linux => entries.retain(|e| matches!(
            e.item.platform.as_str(),
            "aarch64-linux" | "x86_64-linux" | "i686-linux"
        )),
        FailsOn::Darwin => entries.retain(|e| matches!(
            e.item.platform.as_str(),
            "aarch64-darwin" | "x86_64-darwin"
        )),
        FailsOn::Aarch64Linux => entries.retain(|e| e.item.platform == "aarch64-linux"),
        FailsOn::X8664Linux => entries.retain(|e| e.item.platform == "x86_64-linux"),
        FailsOn::Aarch64Darwin => entries.retain(|e| e.item.platform == "aarch64-darwin"),
        FailsOn::X8664Darwin => entries.retain(|e| e.item.platform == "x86_64-darwin"),
        FailsOn::I686Linux => entries.retain(|e| e.item.platform == "i686-linux"),
    }

    // Apply maintainer filter
    if let Some(ref name) = filter.maintainer {
        entries.retain(|e| e.item.maintainers.iter().any(|m| m == name));
    } else if filter.no_maintainer {
        entries.retain(|e| e.item.maintainers.is_empty());
    }

    if let Some(dest) = filter.export {
        table::export_csv(&entries, &dest)?;
    } else {
        table::print_failures(&entries);
    }
    Ok(())
}
