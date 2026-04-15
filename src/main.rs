mod cli;
mod fetcher;
mod table;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, FailsOn, JobFilter};
fn main() -> Result<()> {
    let cli = Cli::parse();

    let (job_filter, filter) = match cli.command {
        Command::Stats => {
            let stats = fetcher::fetch_stats()?;
            table::print_stats(&stats);
            return Ok(());
        }
        Command::All { filter } => (JobFilter::All, filter),
        Command::Direct { filter } => (JobFilter::Direct, filter),
        Command::Indirect { filter } => (JobFilter::Indirect, filter),
    };

    run_failures(job_filter, filter, cli.no_pager)?;
    Ok(())
}

fn run_failures(job_filter: JobFilter, filter: cli::FailureFilter, no_pager: bool) -> Result<()> {
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
        if !no_pager && entries.len() >= 50 {
            // Force ANSI color output — owo-colors detects the pager pipe as
            // non-TTY and strips colors otherwise.
            unsafe { std::env::set_var("CLICOLOR_FORCE", "1") };
            pager::Pager::with_default_pager("less -R").setup();
        }
        table::print_failures(&entries);
    }
    Ok(())
}
