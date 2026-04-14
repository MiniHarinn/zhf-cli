use serde::{Deserialize, Serialize};

/// Top-level stats — fetched by `zhf stats`
#[derive(Debug, Serialize, Deserialize)]
pub struct IndexJson {
    pub generated_at: String,
    /// nixos/unstable evaluation (NixOS tests/modules)
    pub nixos_eval: EvalInfo,
    /// nixpkgs/unstable evaluation (all nixpkgs packages)
    pub nixpkgs_eval: EvalInfo,
    pub counts: FailureCounts,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EvalInfo {
    pub id: u64,
    pub time: String,
}

#[derive(Debug, Serialize, Deserialize)]
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
