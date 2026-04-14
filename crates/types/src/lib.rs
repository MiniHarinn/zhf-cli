use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Top-level index — fetched by `zhf stats`
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexJson {
    pub generated_at: String,
    /// Per-channel data, keyed by slug (e.g. "nixos_unstable", "nixpkgs_staging_next")
    pub channels: HashMap<String, ChannelInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub eval: EvalInfo,
    pub direct_counts: FailureCounts,
    pub indirect_counts: FailureCounts,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EvalInfo {
    pub id: u64,
    pub time: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FailureCounts {
    pub aarch64_darwin: u32,
    pub aarch64_linux: u32,
    pub x86_64_darwin: u32,
    pub x86_64_linux: u32,
    pub i686_linux: u32,
    pub total: u32,
}

/// A single failed build — used in both direct.json and indirect.json
#[derive(Debug, Serialize, Deserialize)]
pub struct FailureItem {
    /// Full display attribute path (e.g. "nixos.tests.foo.x86_64-linux" or "nixpkgs.bar.x86_64-linux")
    pub attrpath: String,
    pub platform: String,
    /// GitHub handles of package maintainers (empty if none)
    pub maintainers: Vec<String>,
    pub hydra_id: u64,
}
