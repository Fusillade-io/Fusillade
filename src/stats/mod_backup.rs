use std::collections::{HashMap, VecDeque};
use tokio::time::Duration;
use hdrhistogram::Histogram;
use serde::{Serialize, Deserialize};
use std::sync::{Arc, RwLock};
use rquickjs::class::Trace;
use rquickjs::JsLifetime;

pub mod html;
pub mod db;
pub mod otlp;

pub type SharedAggregator = Arc<RwLock<StatsAggregator>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestTimings {
    pub blocked: Duration,
    pub connecting: Duration,
    pub tls_handshaking: Duration,
    pub sending: Duration,
    pub waiting: Duration,
    pub receiving: Duration,
    pub duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Metric {
    Request {
        #[allow(dead_code)]
        name: String,
        timings: RequestTimings,
        status: u16,
        error: Option<String>,
    },
    Check {
        name: String,
        success: bool,
    },
    Log {
        message: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct RequestReport {
    pub total_requests: usize,
    pub min_latency_ms: u128,
    pub max_latency_ms: u128,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub error_count: usize,
    // Detailed timing stats (averages for simplicity in report)
    pub avg_blocked_ms: f64,
    pub avg_connecting_ms: f64,
    pub avg_tls_handshaking_ms: f64,
    pub avg_sending_ms: f64,
    pub avg_waiting_ms: f64,
    pub avg_receiving_ms: f64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ReportStats {
    pub total_requests: usize,
    pub total_duration_ms: u128,
    pub avg_latency_ms: f64,
    pub min_latency_ms: u128,
    pub max_latency_ms: u128,
    pub p50_latency_ms: f64,
    pub p90_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub status_codes: HashMap<u16, usize>,
    pub errors: HashMap<String, usize>,
    pub checks: HashMap<String, (usize, usize)>,
    pub grouped_requests: HashMap<String, RequestReport>,
}

pub struct RequestStats {
    pub total_requests: usize,
    pub total_duration: Duration,
    pub min_duration: Option<Duration>,
    pub max_duration: Duration,
    pub histogram: Histogram<u64>,
    pub error_count: usize,
    // Accumulators for averages
    pub total_blocked: Duration,
    pub total_connecting: Duration,
    pub total_tls_handshaking: Duration,
    pub total_sending: Duration,
    pub total_waiting: Duration,
    pub total_receiving: Duration,
}

impl RequestStats {
    pub fn new() -> Self {
        Self {
            total_requests: 0,
            total_duration: Duration::from_secs(0),
            min_duration: None,
            max_duration: Duration::from_secs(0),
            histogram: Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 3).unwrap(),
            error_count: 0,
            total_blocked: Duration::ZERO,
            total_connecting: Duration::ZERO,
            total_tls_handshaking: Duration::ZERO,
            total_sending: Duration::ZERO,
            total_waiting: Duration::ZERO,
            total_receiving: Duration::ZERO,
        }
    }
}

impl Default for RequestStats {
    fn default() -> Self {
        Self::new()
    }
}

pub struct StatsAggregator {
    pub total_requests: usize,
    pub total_duration: Duration,
    pub min_duration: Option<Duration>,
    pub max_duration: Duration,
    pub status_codes: HashMap<u16, usize>,
    pub errors: HashMap<String, usize>,
    pub checks: HashMap<String, (usize, usize)>,
    pub histogram: Histogram<u64>,
    pub requests: HashMap<String, RequestStats>,
    pub logs: VecDeque<String>,
}

unsafe impl<'js> JsLifetime<'js> for StatsAggregator {
    type Changed<'to> = StatsAggregator;
}

impl<'js> Trace<'js> for StatsAggregator {
    fn trace(&self, _tracer: rquickjs::class::Tracer<'_, 'js>) {}
}

impl StatsAggregator {
    pub fn new() -> Self {
        Self {
            total_requests: 0,
            total_duration: Duration::from_secs(0),
            min_duration: None,
            max_duration: Duration::from_secs(0),
            status_codes: HashMap::new(),
            errors: HashMap::new(),
            checks: HashMap::new(),
            histogram: Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 3).unwrap(),
            requests: HashMap::new(),
            logs: VecDeque::new(),
        }
    }

    pub fn add(&mut self, metric: Metric) {
        match metric {
            Metric::Request { timings, status, error, name } => {
                self.total_requests += 1;
                self.total_duration += timings.duration;
                
                if self.min_duration.is_none_or(|min| timings.duration < min) {
                    self.min_duration = Some(timings.duration);
                }
                
                if timings.duration > self.max_duration {
                    self.max_duration = timings.duration;
                }

                // Record to global histogram
                let micros = timings.duration.as_micros() as u64;
                let _ = self.histogram.record(micros.max(1));

                *self.status_codes.entry(status).or_insert(0) += 1;

                if let Some(err) = error.clone() {
                    *self.errors.entry(err).or_insert(0) += 1;
                }

                // Per-request stats
                let req_stats = self.requests.entry(name).or_default();
                req_stats.total_requests += 1;
                req_stats.total_duration += timings.duration;
                if req_stats.min_duration.is_none_or(|min| timings.duration < min) {
                    req_stats.min_duration = Some(timings.duration);
                }
                if timings.duration > req_stats.max_duration {
                    req_stats.max_duration = timings.duration;
                }
                let _ = req_stats.histogram.record(micros.max(1));
                if error.is_some() {
                    req_stats.error_count += 1;
                }
                
                // Aggregate granular timings
                req_stats.total_blocked += timings.blocked;
                req_stats.total_connecting += timings.connecting;
                req_stats.total_tls_handshaking += timings.tls_handshaking;
                req_stats.total_sending += timings.sending;
                req_stats.total_waiting += timings.waiting;
                req_stats.total_receiving += timings.receiving;
            }
            Metric::Check { name, success } => {
                let entry = self.checks.entry(name).or_insert((0, 0));
                entry.0 += 1;
                if success {
                    entry.1 += 1;
                }
            }
            Metric::Log { message } => {
                self.logs.push_back(message);
                if self.logs.len() > 100 {
                    self.logs.pop_front();
                }
            }
        }
    }

    pub fn to_report(&self) -> ReportStats {
        let avg_latency = if self.total_requests > 0 {
            self.total_duration.as_millis() as f64 / self.total_requests as f64
        } else {
            0.0
        };

        let mut grouped_requests = HashMap::new();
        for (name, stats) in &self.requests {
            let req_avg = if stats.total_requests > 0 {
                stats.total_duration.as_millis() as f64 / stats.total_requests as f64
            } else {
                0.0
            };

            let count = stats.total_requests as f64;
            let (avg_blocked, avg_conn, avg_tls, avg_send, avg_wait, avg_recv) = if stats.total_requests > 0 {
                 (
                    stats.total_blocked.as_millis() as f64 / count,
                    stats.total_connecting.as_millis() as f64 / count,
                    stats.total_tls_handshaking.as_millis() as f64 / count,
                    stats.total_sending.as_millis() as f64 / count,
                    stats.total_waiting.as_millis() as f64 / count,
                    stats.total_receiving.as_millis() as f64 / count
                 )
            } else {
                (0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
            };

            grouped_requests.insert(name.clone(), RequestReport {
                total_requests: stats.total_requests,
                min_latency_ms: stats.min_duration.unwrap_or_default().as_millis(),
                max_latency_ms: stats.max_duration.as_millis(),
                avg_latency_ms: req_avg,
                p95_latency_ms: Duration::from_micros(stats.histogram.value_at_quantile(0.95)).as_secs_f64() * 1000.0,
                p99_latency_ms: Duration::from_micros(stats.histogram.value_at_quantile(0.99)).as_secs_f64() * 1000.0,
                error_count: stats.error_count,
                avg_blocked_ms: avg_blocked,
                avg_connecting_ms: avg_conn,
                avg_tls_handshaking_ms: avg_tls,
                avg_sending_ms: avg_send,
                avg_waiting_ms: avg_wait,
                avg_receiving_ms: avg_recv,
            });
        }

        ReportStats {
            total_requests: self.total_requests,
            total_duration_ms: self.total_duration.as_millis(),
            avg_latency_ms: avg_latency,
            min_latency_ms: self.min_duration.unwrap_or_default().as_millis(),
            max_latency_ms: self.max_duration.as_millis(),
            p50_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.5)).as_secs_f64() * 1000.0,
            p90_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.9)).as_secs_f64() * 1000.0,
            p95_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.95)).as_secs_f64() * 1000.0,
            p99_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.99)).as_secs_f64() * 1000.0,
            status_codes: self.status_codes.clone(),
            errors: self.errors.clone(),
            checks: self.checks.clone(),
            grouped_requests,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_report()).unwrap_or_default()
    }

    pub fn report(&self) {
        if self.total_requests == 0 && self.checks.is_empty() {
            println!("\n--- Test Summary ---");
            println!("No metrics collected.");
            return;
        }

        println!("\n--- Test Summary ---");
        
        if self.total_requests > 0 {
            let avg_duration = self.total_duration / self.total_requests as u32;
            println!("Total Requests: {}", self.total_requests);
            println!("Avg Latency:    {:?}", avg_duration);
            println!("Min Latency:    {:?}", self.min_duration.unwrap_or_default());
            println!("Max Latency:    {:?}", self.max_duration);
            println!("P50 Latency:    {:?}", Duration::from_micros(self.histogram.value_at_quantile(0.5)));
            println!("P90 Latency:    {:?}", Duration::from_micros(self.histogram.value_at_quantile(0.9)));
            println!("P95 Latency:    {:?}", Duration::from_micros(self.histogram.value_at_quantile(0.95)));
            println!("P99 Latency:    {:?}", Duration::from_micros(self.histogram.value_at_quantile(0.99)));
            
            println!("\nStatus Codes:");
            let mut codes: Vec<_> = self.status_codes.iter().collect();
            codes.sort_by_key(|a| a.0);
            for (code, count) in codes {
                println!("  {}: {}", code, count);
            }
        }

        if !self.requests.is_empty() {
            println!("\nGrouped Requests:");
            let mut groups: Vec<_> = self.requests.iter().collect();
            groups.sort_by_key(|a| a.0);
            for (name, stats) in groups {
                let p95 = Duration::from_micros(stats.histogram.value_at_quantile(0.95));
                println!("  Request: {}", name);
                println!("    Count: {}", stats.total_requests);
                println!("    P95:   {:?}", p95);
                if stats.error_count > 0 {
                    println!("    Errors: {}", stats.error_count);
                }
            }
        }

        if !self.errors.is_empty() {
            println!("\nErrors:");
            for (err, count) in &self.errors {
                println!("  {}: {}", err, count);
            }
        }

        if !self.checks.is_empty() {
            println!("\nChecks:");
            let mut checks: Vec<_> = self.checks.iter().collect();
            checks.sort_by_key(|a| a.0);
            for (name, (total, passes)) in checks {
                let fail = total - passes;
                let percent = (*passes as f64 / *total as f64) * 100.0;
                if fail > 0 {
                    println!("  ✓ {} : {:.2}% ({} passed, {} failed)", name, percent, passes, fail);
                } else {
                    println!("  ✓ {} : 100% ({} passed)", name, passes);
                }
            }
        }
        println!("--------------------\n");
    }

    #[allow(dead_code)]
    pub fn validate_thresholds(&self, criteria: &HashMap<String, Vec<String>>) -> Vec<String> {
        let mut failures = Vec::new();
        let report = self.to_report();

        for (metric_name, thresholds) in criteria {
            for threshold in thresholds {
                let parts: Vec<&str> = threshold.split_whitespace().collect();
                if parts.len() < 3 { continue; }

                let check_type = parts[0];
                let operator = parts[1];
                let value: f64 = parts[2].parse().unwrap_or(0.0);

                let actual_value = match metric_name.as_str() {
                    "http_req_duration" => {
                        match check_type {
                            "p95" => report.p95_latency_ms,
                            "p99" => report.p99_latency_ms,
                            "avg" => report.avg_latency_ms,
                            "max" => report.max_latency_ms as f64,
                            "min" => report.min_latency_ms as f64,
                            _ => 0.0,
                        }
                    },
                    "http_req_failed" => {
                        if check_type == "rate" {
                            if report.total_requests > 0 {
                                let total_errors: usize = report.errors.values().sum::<usize>() + report.grouped_requests.values().map(|r| r.error_count).sum::<usize>();
                                total_errors as f64 / report.total_requests as f64
                            } else {
                                0.0
                            }
                        } else {
                            0.0
                        }
                    },
                    _ => 0.0,
                };

                let pass = match operator {
                    "<" => actual_value < value,
                    "<=" => actual_value <= value,
                    ">" => actual_value > value,
                    ">=" => actual_value >= value,
                    "==" => (actual_value - value).abs() < f64::EPSILON,
                    _ => false,
                };

                if !pass {
                    failures.push(format!("Threshold FAILED: {} {} {} (actual: {:.2})", metric_name, check_type, threshold, actual_value));
                }
            }
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    #[test]
    fn test_aggregator_math() {
        let mut agg = StatsAggregator::new();
        
        agg.add(Metric::Request {
            name: "test".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(100),
                ..Default::default()
            },
            status: 200,
            error: None,
        });
        
        agg.add(Metric::Request {
            name: "test".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(200),
                ..Default::default()
            },
            status: 200,
            error: None,
        });

        assert_eq!(agg.total_requests, 2);
        assert_eq!(agg.total_duration, Duration::from_millis(300));
        assert_eq!(agg.min_duration, Some(Duration::from_millis(100)));
        assert_eq!(agg.max_duration, Duration::from_millis(200));
        assert_eq!(*agg.status_codes.get(&200).unwrap(), 2);
    }

    #[test]
    fn test_aggregator_errors() {
        let mut agg = StatsAggregator::new();
        
        agg.add(Metric::Request {
            name: "test".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(10),
                ..Default::default()
            },
            status: 0,
            error: Some("Timeout".to_string()),
        });

        assert_eq!(agg.total_requests, 1);
        assert_eq!(*agg.errors.get("Timeout").unwrap(), 1);
    }
    
    #[test]
    fn test_aggregator_checks() {
        let mut agg = StatsAggregator::new();
        
        agg.add(Metric::Check { name: "status is 200".to_string(), success: true });
        agg.add(Metric::Check { name: "status is 200".to_string(), success: false });
        
        let (total, passes) = agg.checks.get("status is 200").unwrap();
        assert_eq!(*total, 2);
        assert_eq!(*passes, 1);
    }

    #[test]
    fn test_aggregator_histogram() {
        let mut agg = StatsAggregator::new();
        
        // Add 100 requests with duration i * 1ms
        for i in 1..=100 {
            agg.add(Metric::Request {
                name: "test".to_string(),
                timings: RequestTimings {
                    duration: Duration::from_millis(i),
                    ..Default::default()
                },
                status: 200,
                error: None,
            });
        }

        // P50 should be around 50ms (50000us)
        let p50 = agg.histogram.value_at_quantile(0.5);
        assert!(p50 >= 49000 && p50 <= 51000, "P50 was {}", p50);
        
        // P99 should be around 99ms (99000us)
        let p99 = agg.histogram.value_at_quantile(0.99);
        assert!(p99 >= 98000 && p99 <= 100000, "P99 was {}", p99);
    }

    #[test]
    fn test_report_generation() {
        let mut agg = StatsAggregator::new();
        agg.add(Metric::Request {
            name: "req1".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(100),
                ..Default::default()
            },
            status: 200,
            error: None,
        });
        
        let report = agg.to_report();
        assert_eq!(report.total_requests, 1);
        assert_eq!(report.avg_latency_ms, 100.0);
        
        let json = agg.to_json();
        assert!(json.contains("\"total_requests\": 1"));
        assert!(json.contains("\"avg_latency_ms\": 100.0"));
        
        let html = crate::stats::html::generate_html(&report);
        assert!(html.contains("Thruster Load Test Report"));
        assert!(html.contains("100.00 ms"));
    }

    #[test]
    fn test_validate_criteria() {
        let mut agg = StatsAggregator::new();
        
        // Add some successful requests
        for i in 1..=100 {
            agg.add(Metric::Request {
                name: "api".to_string(),
                timings: RequestTimings {
                    duration: Duration::from_millis(i),
                    ..Default::default()
                },
                status: 200,
                error: None,
            });
        }

        let mut criteria = HashMap::new();
        // Should pass: p95 is ~95ms, which is < 200
        criteria.insert("http_req_duration".to_string(), vec!["p95 < 200".to_string()]);
        // Should pass: rate is 0, which is < 0.1
        criteria.insert("http_req_failed".to_string(), vec!["rate < 0.1".to_string()]);

        let failures = agg.validate_thresholds(&criteria);
        assert!(failures.is_empty(), "Expected no failures, got: {:?}", failures);

        // Test failing criteria
        let mut fail_criteria = HashMap::new();
        fail_criteria.insert("http_req_duration".to_string(), vec!["p95 < 50".to_string()]);
        
        let failures = agg.validate_thresholds(&fail_criteria);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("FAILED"));
    }

    #[test]
    fn test_criteria_operators() {
        let mut agg = StatsAggregator::new();
        agg.add(Metric::Request {
            name: "api".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(100),
                ..Default::default()
            },
            status: 200,
            error: None,
        });

        let mut c = HashMap::new();
        c.insert("http_req_duration".to_string(), vec!["avg <= 100".to_string()]);
        assert!(agg.validate_thresholds(&c).is_empty());

        let mut c = HashMap::new();
        c.insert("http_req_duration".to_string(), vec!["avg >= 100".to_string()]);
        assert!(agg.validate_thresholds(&c).is_empty());

        let mut c = HashMap::new();
        c.insert("http_req_duration".to_string(), vec!["avg > 100".to_string()]);
        assert_eq!(agg.validate_thresholds(&c).len(), 1);
    }

    #[test]
    fn test_report_empty() {
        let agg = StatsAggregator::new();
        let report = agg.to_report();
        
        assert_eq!(report.total_requests, 0);
        assert_eq!(report.avg_latency_ms, 0.0);
        assert_eq!(report.p99_latency_ms, 0.0);
        
        let html = crate::stats::html::generate_html(&report);
        assert!(html.contains("No requests made"));
    }
}
