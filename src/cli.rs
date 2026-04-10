use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "zhf", about = "Zero Hydra Failures CLI - scrape zh.fail", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show upstream status (target, evals, platform counts)
    Stats,
    /// Show problematic dependencies from the homepage
    Problematic {
        #[arg(long, value_name = "FILE", help = "Export as CSV instead of displaying")]
        export: Option<String>,
    },
    /// Show all failed builds (direct + indirect)
    All {
        #[command(flatten)]
        filter: FailureFilter,
    },
    /// Show direct failures (packages that fail to build themselves)
    Direct {
        #[command(flatten)]
        filter: FailureFilter,
    },
    /// Show indirect failures (packages whose dependency failed)
    Indirect {
        #[command(flatten)]
        filter: FailureFilter,
    },
}

#[derive(Args)]
pub struct FailureFilter {
    /// Filter to nixpkgs.* attrpaths only
    #[arg(long, conflicts_with = "nixos")]
    pub nixpkgs: bool,

    /// Filter to nixos.* attrpaths only
    #[arg(long, conflicts_with = "nixpkgs")]
    pub nixos: bool,

    /// Filter by platform
    #[arg(long, value_enum, default_value = "all")]
    pub fails_on: FailsOn,

    /// Filter by maintainer name
    #[arg(long, value_name = "NAME", conflicts_with = "no_maintainer")]
    pub maintainer: Option<String>,

    /// Show only packages with no maintainer
    #[arg(long, conflicts_with = "maintainer")]
    pub no_maintainer: bool,

    /// Export as CSV to FILE instead of displaying the table
    #[arg(long, value_name = "FILE")]
    pub export: Option<String>,
}

#[derive(ValueEnum, Clone, PartialEq)]
pub enum FailsOn {
    All,
    Linux,
    Darwin,
    #[value(name = "aarch64-linux")]
    Aarch64Linux,
    #[value(name = "x86_64-linux")]
    X8664Linux,
    #[value(name = "aarch64-darwin")]
    Aarch64Darwin,
    #[value(name = "x86_64-darwin")]
    X8664Darwin,
}

#[derive(Clone, PartialEq)]
pub enum JobFilter {
    All,
    Direct,
    Indirect,
}
