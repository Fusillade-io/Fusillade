use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
pub struct ScheduleStep {
    pub duration: String,
    pub target: usize,
}

/// Configuration for a single scenario within multi-scenario tests
#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
pub struct ScenarioConfig {
    /// Executor type (constant-vus, ramping-vus, constant-arrival-rate, per-vu-iterations)
    pub executor: Option<String>,
    /// Number of concurrent workers (k6: vus)
    pub workers: Option<usize>,
    /// Duration of the scenario
    pub duration: Option<String>,
    /// Fixed iterations per worker (for per-vu-iterations executor)
    pub iterations: Option<u64>,
    /// Ramping schedule (k6: stages)
    pub schedule: Option<Vec<ScheduleStep>>,
    /// Rate for arrival-rate executors
    pub rate: Option<u64>,
    /// Time unit for rate
    pub time_unit: Option<String>,
    /// Function name to execute (default: "default")
    pub exec: Option<String>,
    /// Delay before starting this scenario (e.g., "30s")
    #[serde(alias = "startTime")]
    pub start_time: Option<String>,
    /// Per-scenario thresholds (k6: thresholds)
    #[serde(alias = "thresholds")]
    pub thresholds: Option<HashMap<String, Vec<String>>>,
    /// Worker thread stack size in bytes (default: 128KB). Increase if encountering stack overflows.
    pub stack_size: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, JsonSchema)]
pub struct Config {
    /// Number of concurrent workers (VUs)
    pub workers: Option<usize>,
    /// Duration of the test (e.g., "10s", "1m")
    pub duration: Option<String>,
    /// Ramping schedule (stages)
    pub schedule: Option<Vec<ScheduleStep>>,
    /// Executor type (constant-arrival-rate, ramping-vus, etc.)
    pub executor: Option<String>,
    /// Rate for arrival-rate executors
    pub rate: Option<u64>,
    /// Time unit for rate (e.g., "1s", "1m")
    pub time_unit: Option<String>,
    /// Pass/Fail criteria (thresholds)
    pub criteria: Option<HashMap<String, Vec<String>>>, 
    /// Minimum time per iteration
    pub min_iteration_duration: Option<String>,
    /// Target URL to warmup connections before starting
    pub warmup: Option<String>,
    /// Graceful shutdown wait time
    pub stop: Option<String>,
    /// Fixed number of iterations per worker (per-vu-iterations executor)
    pub iterations: Option<u64>,
    /// Multiple scenarios with independent configs
    pub scenarios: Option<HashMap<String, ScenarioConfig>>,
    /// Chaos injection: Jitter duration (e.g., "500ms")
    pub jitter: Option<String>,
    /// Chaos injection: Packet drop probability (0.0 - 1.0)
    pub drop: Option<f64>,
    /// Worker thread stack size in bytes (default: 128KB). Increase to 256KB or higher if encountering stack overflows.
    pub stack_size: Option<usize>,
}

impl Config {
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::schema_for;

    #[test]
    fn test_config_schema() {
        let schema = schema_for!(Config);
        let schema_json = serde_json::to_string(&schema).unwrap();
        assert!(schema_json.contains("workers"));
        assert!(schema_json.contains("schedule"));
        assert!(schema_json.contains("criteria"));
    }
}
