use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
    /// Worker thread stack size in bytes (default: 32KB). Increase if encountering stack overflows.
    pub stack_size: Option<usize>,
    /// Sink (discard) response bodies to save memory. Body is still downloaded from the network
    /// (required for connection keep-alive), but immediately discarded instead of being stored.
    /// When enabled, response.body will be null. Useful for high-throughput tests
    /// where response content is not needed for assertions.
    #[serde(alias = "responseSink")]
    pub response_sink: Option<bool>,
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
    /// Worker thread stack size in bytes (default: 32KB). Increase if encountering stack overflows with complex scripts.
    pub stack_size: Option<usize>,
    /// Sink (discard) response bodies to save memory. Body is still downloaded from the network
    /// (required for connection keep-alive), but immediately discarded instead of being stored.
    /// When enabled, response.body will be null. Useful for high-throughput tests
    /// where response content is not needed for assertions.
    #[serde(alias = "responseSink")]
    pub response_sink: Option<bool>,
    /// Disable per-endpoint (per-URL) metrics tracking. When enabled, only aggregate
    /// metrics are collected, reducing memory usage for high-cardinality URL patterns.
    /// Endpoint tracking is ON by default.
    #[serde(alias = "noEndpointTracking")]
    pub no_endpoint_tracking: Option<bool>,
    /// Abort the test immediately if any threshold is breached. Useful for CI/CD to fail fast.
    /// When enabled, the test will exit with a non-zero status code on threshold failure.
    #[serde(alias = "abortOnFail")]
    pub abort_on_fail: Option<bool>,
    /// Enable memory-safe mode: throttle worker spawning if memory usage is high.
    /// When system memory exceeds 85%, new worker spawning is paused.
    /// When memory exceeds 95%, existing workers are gradually stopped.
    #[serde(alias = "memorySafe")]
    pub memory_safe: Option<bool>,
}

impl Config {}

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

    #[test]
    fn test_config_deserialize_minimal() {
        let yaml = r#"
workers: 10
duration: "30s"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.workers, Some(10));
        assert_eq!(config.duration, Some("30s".to_string()));
    }

    #[test]
    fn test_config_deserialize_with_schedule() {
        let yaml = r#"
schedule:
  - duration: "10s"
    target: 5
  - duration: "20s"
    target: 10
  - duration: "10s"
    target: 0
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let schedule = config.schedule.unwrap();
        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule[0].target, 5);
        assert_eq!(schedule[1].target, 10);
        assert_eq!(schedule[2].target, 0);
    }

    #[test]
    fn test_config_deserialize_with_criteria() {
        let yaml = r#"
criteria:
  http_req_duration:
    - "p(95) < 500"
    - "avg < 200"
  checks:
    - "rate > 0.95"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let criteria = config.criteria.unwrap();
        assert!(criteria.contains_key("http_req_duration"));
        assert!(criteria.contains_key("checks"));
        assert_eq!(criteria["http_req_duration"].len(), 2);
    }

    #[test]
    fn test_config_deserialize_chaos() {
        let yaml = r#"
jitter: "100ms"
drop: 0.1
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.jitter, Some("100ms".to_string()));
        assert_eq!(config.drop, Some(0.1));
    }

    #[test]
    fn test_config_deserialize_arrival_rate() {
        let yaml = r#"
executor: constant-arrival-rate
rate: 100
time_unit: "1s"
duration: "1m"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.executor, Some("constant-arrival-rate".to_string()));
        assert_eq!(config.rate, Some(100));
        assert_eq!(config.time_unit, Some("1s".to_string()));
    }

    #[test]
    fn test_config_deserialize_multi_scenario() {
        let yaml = r#"
scenarios:
  login:
    workers: 5
    duration: "30s"
    exec: "loginFlow"
  browse:
    workers: 20
    duration: "1m"
    start_time: "30s"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let scenarios = config.scenarios.unwrap();
        assert!(scenarios.contains_key("login"));
        assert!(scenarios.contains_key("browse"));
        assert_eq!(scenarios["login"].workers, Some(5));
        assert_eq!(scenarios["browse"].start_time, Some("30s".to_string()));
    }

    #[test]
    fn test_config_deserialize_json() {
        let json = r#"{
            "workers": 5,
            "duration": "10s",
            "iterations": 100
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.workers, Some(5));
        assert_eq!(config.iterations, Some(100));
    }

    #[test]
    fn test_scenario_config_defaults() {
        let config = ScenarioConfig::default();
        assert!(config.workers.is_none());
        assert!(config.duration.is_none());
        assert!(config.executor.is_none());
    }

    #[test]
    fn test_config_serialize_roundtrip() {
        let config = Config {
            workers: Some(10),
            duration: Some("30s".to_string()),
            drop: Some(0.05),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(config.workers, parsed.workers);
        assert_eq!(config.duration, parsed.duration);
        assert_eq!(config.drop, parsed.drop);
    }

    #[test]
    fn test_config_response_sink() {
        let yaml = r#"
workers: 10
duration: "30s"
response_sink: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.response_sink, Some(true));
    }

    #[test]
    fn test_config_response_sink_camel_case() {
        let json = r#"{
            "workers": 5,
            "duration": "10s",
            "responseSink": true
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.response_sink, Some(true));
    }

    #[test]
    fn test_scenario_config_response_sink() {
        let yaml = r#"
scenarios:
  load_test:
    workers: 100
    duration: "1m"
    response_sink: true
  normal_test:
    workers: 10
    duration: "30s"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let scenarios = config.scenarios.unwrap();
        assert_eq!(scenarios["load_test"].response_sink, Some(true));
        assert_eq!(scenarios["normal_test"].response_sink, None);
    }

    #[test]
    fn test_config_abort_on_fail() {
        let yaml = r#"
workers: 10
duration: "30s"
abort_on_fail: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.abort_on_fail, Some(true));
    }

    #[test]
    fn test_config_abort_on_fail_camel_case() {
        let json = r#"{
            "workers": 5,
            "duration": "10s",
            "abortOnFail": true
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.abort_on_fail, Some(true));
    }

    #[test]
    fn test_config_memory_safe() {
        let yaml = r#"
workers: 10000
duration: "5m"
memory_safe: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.memory_safe, Some(true));
    }

    #[test]
    fn test_config_memory_safe_camel_case() {
        let json = r#"{
            "workers": 5000,
            "duration": "5m",
            "memorySafe": true
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.memory_safe, Some(true));
    }

    #[test]
    fn test_config_no_endpoint_tracking() {
        let yaml = r#"
workers: 10
duration: "30s"
no_endpoint_tracking: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.no_endpoint_tracking, Some(true));
    }

    #[test]
    fn test_config_no_endpoint_tracking_camel_case() {
        let json = r#"{
            "workers": 5,
            "duration": "10s",
            "noEndpointTracking": true
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.no_endpoint_tracking, Some(true));
    }

    #[test]
    fn test_config_all_fields() {
        let yaml = r#"
workers: 50
duration: "2m"
schedule:
  - duration: "30s"
    target: 25
  - duration: "1m"
    target: 50
executor: "ramping-vus"
rate: 100
time_unit: "1s"
criteria:
  http_req_duration:
    - "p95<500"
min_iteration_duration: "1s"
warmup: "https://api.example.com/health"
stop: "10s"
iterations: 100
jitter: "100ms"
drop: 0.05
stack_size: 65536
response_sink: true
no_endpoint_tracking: false
abort_on_fail: true
memory_safe: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.workers, Some(50));
        assert_eq!(config.duration, Some("2m".to_string()));
        assert!(config.schedule.is_some());
        assert_eq!(config.schedule.as_ref().unwrap().len(), 2);
        assert_eq!(config.executor, Some("ramping-vus".to_string()));
        assert_eq!(config.rate, Some(100));
        assert_eq!(config.time_unit, Some("1s".to_string()));
        assert!(config.criteria.is_some());
        assert_eq!(config.min_iteration_duration, Some("1s".to_string()));
        assert_eq!(
            config.warmup,
            Some("https://api.example.com/health".to_string())
        );
        assert_eq!(config.stop, Some("10s".to_string()));
        assert_eq!(config.iterations, Some(100));
        assert_eq!(config.jitter, Some("100ms".to_string()));
        assert_eq!(config.drop, Some(0.05));
        assert_eq!(config.stack_size, Some(65536));
        assert_eq!(config.response_sink, Some(true));
        assert_eq!(config.no_endpoint_tracking, Some(false));
        assert_eq!(config.abort_on_fail, Some(true));
        assert_eq!(config.memory_safe, Some(true));
    }

    #[test]
    fn test_config_merge_priority() {
        // Test that merging works correctly (simulating CLI > file > script priority)
        let script_config = Config {
            workers: Some(10),
            duration: Some("30s".to_string()),
            jitter: Some("100ms".to_string()),
            ..Default::default()
        };

        let file_config = Config {
            workers: Some(50),                // Should override script
            duration: Some("1m".to_string()), // Should override script
            drop: Some(0.05),                 // New field from file
            ..Default::default()
        };

        // Simulate merging: file overrides script
        let mut merged = script_config.clone();
        if file_config.workers.is_some() {
            merged.workers = file_config.workers;
        }
        if file_config.duration.is_some() {
            merged.duration = file_config.duration;
        }
        if file_config.drop.is_some() {
            merged.drop = file_config.drop;
        }

        assert_eq!(merged.workers, Some(50)); // From file
        assert_eq!(merged.duration, Some("1m".to_string())); // From file
        assert_eq!(merged.jitter, Some("100ms".to_string())); // From script (not in file)
        assert_eq!(merged.drop, Some(0.05)); // From file

        // Simulate CLI override
        let cli_workers = Some(100);
        if cli_workers.is_some() {
            merged.workers = cli_workers;
        }
        assert_eq!(merged.workers, Some(100)); // From CLI
    }

    #[test]
    fn test_scenario_config_all_fields() {
        let yaml = r#"
scenarios:
  test:
    executor: "constant-vus"
    workers: 10
    duration: "1m"
    iterations: 50
    rate: 10
    time_unit: "1s"
    exec: "testFunction"
    start_time: "30s"
    stack_size: 32768
    response_sink: true
    thresholds:
      http_req_duration:
        - "p95<500"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let scenario = &config.scenarios.unwrap()["test"];
        assert_eq!(scenario.executor, Some("constant-vus".to_string()));
        assert_eq!(scenario.workers, Some(10));
        assert_eq!(scenario.duration, Some("1m".to_string()));
        assert_eq!(scenario.iterations, Some(50));
        assert_eq!(scenario.rate, Some(10));
        assert_eq!(scenario.time_unit, Some("1s".to_string()));
        assert_eq!(scenario.exec, Some("testFunction".to_string()));
        assert_eq!(scenario.start_time, Some("30s".to_string()));
        assert_eq!(scenario.stack_size, Some(32768));
        assert_eq!(scenario.response_sink, Some(true));
        assert!(scenario.thresholds.is_some());
    }
}
