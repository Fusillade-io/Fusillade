use hdrhistogram::Histogram;
use parking_lot::RwLock as ParkingLotRwLock;
use rquickjs::class::Trace;
use rquickjs::JsLifetime;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::time::Duration;

pub mod csv;
pub mod db;
pub mod html;
pub mod otlp;

// SharedAggregator uses std::sync::RwLock for JS bridge compatibility (JsLifetime trait)
pub type SharedAggregator = Arc<std::sync::RwLock<StatsAggregator>>;

/// Sharded aggregator for reduced lock contention at high concurrency
/// Metrics are distributed across N shards based on worker_id % num_shards
/// Uses parking_lot::RwLock for faster lock acquisition under contention
pub struct ShardedAggregator {
    shards: Vec<ParkingLotRwLock<StatsAggregator>>,
    num_shards: usize,
}

impl ShardedAggregator {
    pub fn new(num_shards: usize) -> Self {
        let shards = (0..num_shards)
            .map(|_| ParkingLotRwLock::new(StatsAggregator::new()))
            .collect();
        Self { shards, num_shards }
    }

    /// Add a metric to the appropriate shard based on worker_id
    pub fn add(&self, worker_id: usize, metric: Metric) {
        let shard_idx = worker_id % self.num_shards;
        let mut agg = self.shards[shard_idx].write();
        agg.add(metric);
    }

    /// Merge all shards into a single aggregator for final reporting
    pub fn merge(&self) -> StatsAggregator {
        let mut merged = StatsAggregator::new();

        for shard in &self.shards {
            let shard_data = shard.read();
            // Merge basic stats
            merged.total_requests += shard_data.total_requests;
            merged.total_duration += shard_data.total_duration;
            merged.total_data_sent += shard_data.total_data_sent;
            merged.total_data_received += shard_data.total_data_received;

            // Merge min/max
            if let Some(shard_min) = shard_data.min_duration {
                if merged.min_duration.is_none() || Some(shard_min) < merged.min_duration {
                    merged.min_duration = Some(shard_min);
                }
            }
            if shard_data.max_duration > merged.max_duration {
                merged.max_duration = shard_data.max_duration;
            }

            // Merge status codes
            for (code, count) in &shard_data.status_codes {
                *merged.status_codes.entry(*code).or_insert(0) += count;
            }

            // Merge errors
            for (err, count) in &shard_data.errors {
                *merged.errors.entry(err.clone()).or_insert(0) += count;
            }

            // Merge checks
            for (name, (total, passes)) in &shard_data.checks {
                let entry = merged.checks.entry(name.clone()).or_insert((0, 0));
                entry.0 += total;
                entry.1 += passes;
            }

            // Merge histogram (add all recorded values)
            merged.histogram.add(&shard_data.histogram).ok();

            // Merge per-request stats
            for (name, stats) in &shard_data.requests {
                let merged_stats = merged.requests.entry(name.clone()).or_default();
                merged_stats.total_requests += stats.total_requests;
                merged_stats.total_duration += stats.total_duration;
                if merged_stats.min_duration.is_none()
                    || stats.min_duration < merged_stats.min_duration
                {
                    merged_stats.min_duration = stats.min_duration;
                }
                if stats.max_duration > merged_stats.max_duration {
                    merged_stats.max_duration = stats.max_duration;
                }
                merged_stats.error_count += stats.error_count;
                merged_stats.histogram.add(&stats.histogram).ok();
                merged_stats.total_blocked += stats.total_blocked;
                merged_stats.total_connecting += stats.total_connecting;
                merged_stats.total_tls_handshaking += stats.total_tls_handshaking;
                merged_stats.total_sending += stats.total_sending;
                merged_stats.total_waiting += stats.total_waiting;
                merged_stats.total_receiving += stats.total_receiving;
                merged_stats.total_response_size += stats.total_response_size;
                merged_stats.total_request_size += stats.total_request_size;
            }

            // Merge custom histograms
            for (name, hist) in &shard_data.histograms {
                let merged_hist = merged.histograms.entry(name.clone()).or_insert_with(|| {
                    Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 2).unwrap()
                });
                merged_hist.add(hist).ok();
            }
            for (name, sum) in &shard_data.histogram_sums {
                *merged.histogram_sums.entry(name.clone()).or_insert(0.0) += sum;
            }

            // Merge rates
            for (name, (total, success)) in &shard_data.rates {
                let entry = merged.rates.entry(name.clone()).or_insert((0, 0));
                entry.0 += total;
                entry.1 += success;
            }

            // Merge counters
            for (name, val) in &shard_data.counters {
                *merged.counters.entry(name.clone()).or_insert(0.0) += val;
            }

            // Merge gauges (take the last value from any shard)
            for (name, val) in &shard_data.gauges {
                merged.gauges.insert(name.clone(), *val);
            }

            // Merge logs
            for log in &shard_data.logs {
                merged.logs.push_back(log.clone());
            }
        }

        // Trim logs to max
        while merged.logs.len() > 100 {
            merged.logs.pop_front();
        }

        merged
    }
}

pub type SharedShardedAggregator = Arc<ShardedAggregator>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct RequestTimings {
    pub blocked: Duration,
    pub connecting: Duration,
    pub tls_handshaking: Duration,
    pub sending: Duration,
    pub waiting: Duration,
    pub receiving: Duration,
    pub duration: Duration,
    pub response_size: usize,
    pub request_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Metric {
    Request {
        #[allow(dead_code)]
        name: String,
        timings: RequestTimings,
        status: u16,
        error: Option<String>,
        tags: HashMap<String, String>,
    },
    Check {
        name: String,
        success: bool,
    },
    Log {
        message: String,
    },
    // Custom Metrics
    Histogram {
        name: String,
        value: f64,
        tags: HashMap<String, String>,
    },
    Rate {
        name: String,
        success: bool,
        tags: HashMap<String, String>,
    },
    Counter {
        name: String,
        value: f64,
        tags: HashMap<String, String>,
    },
    Gauge {
        name: String,
        value: f64,
        tags: HashMap<String, String>,
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
    pub avg_response_size: f64,
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
    pub total_data_sent: u64,
    pub total_data_received: u64,
    // Custom Metrics
    pub histograms: HashMap<String, HistogramReport>,
    pub rates: HashMap<String, RateReport>,
    pub counters: HashMap<String, f64>,
    pub gauges: HashMap<String, f64>,
}

#[derive(Serialize, Deserialize)]
pub struct HistogramReport {
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub count: u64,
}

#[derive(Serialize, Deserialize)]
pub struct RateReport {
    pub total: usize,
    pub success: usize,
    pub rate: f64,
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
    pub total_response_size: u64,
    pub total_request_size: u64,
}

impl RequestStats {
    pub fn new() -> Self {
        Self {
            total_requests: 0,
            total_duration: Duration::from_secs(0),
            min_duration: None,
            max_duration: Duration::from_secs(0),
            histogram: Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 2).unwrap(),
            error_count: 0,
            total_blocked: Duration::ZERO,
            total_connecting: Duration::ZERO,
            total_tls_handshaking: Duration::ZERO,
            total_sending: Duration::ZERO,
            total_waiting: Duration::ZERO,
            total_receiving: Duration::ZERO,
            total_response_size: 0,
            total_request_size: 0,
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
    pub total_data_sent: u64,
    pub total_data_received: u64,
    pub logs: VecDeque<String>,
    // Custom Metrics Storage
    pub histograms: HashMap<String, Histogram<u64>>,
    pub histogram_sums: HashMap<String, f64>, // To calculate avg
    pub rates: HashMap<String, (usize, usize)>, // (total, success)
    pub counters: HashMap<String, f64>,
    pub gauges: HashMap<String, f64>,
}

unsafe impl<'js> JsLifetime<'js> for StatsAggregator {
    type Changed<'to> = StatsAggregator;
}

impl<'js> Trace<'js> for StatsAggregator {
    fn trace(&self, _tracer: rquickjs::class::Tracer<'_, 'js>) {}
}

impl Default for StatsAggregator {
    fn default() -> Self {
        Self::new()
    }
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
            histogram: Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 2).unwrap(),
            requests: HashMap::new(),
            total_data_sent: 0,
            total_data_received: 0,
            logs: VecDeque::new(),
            histograms: HashMap::new(),
            histogram_sums: HashMap::new(),
            rates: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
        }
    }

    pub fn add(&mut self, metric: Metric) {
        match metric {
            Metric::Request {
                timings,
                status,
                error,
                name,
                tags,
            } => {
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

                // Append tags to name for granular aggregation
                let mut full_name = name;
                if !tags.is_empty() {
                    let mut sorted_tags: Vec<_> = tags.iter().collect();
                    sorted_tags.sort_by_key(|a| a.0);
                    full_name.push('{');
                    for (i, (k, v)) in sorted_tags.iter().enumerate() {
                        if i > 0 {
                            full_name.push(',');
                        }
                        full_name.push_str(k);
                        full_name.push(':');
                        full_name.push_str(v);
                    }
                    full_name.push('}');
                }

                // Per-request stats
                let req_stats = self.requests.entry(full_name).or_default();
                req_stats.total_requests += 1;
                req_stats.total_duration += timings.duration;
                if req_stats
                    .min_duration
                    .is_none_or(|min| timings.duration < min)
                {
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
                req_stats.total_response_size += timings.response_size as u64;
                req_stats.total_request_size += timings.request_size as u64;

                // Update global data counters
                self.total_data_sent += timings.request_size as u64;
                self.total_data_received += timings.response_size as u64;
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
            Metric::Histogram {
                name,
                value,
                tags: _,
            } => {
                let h = self.histograms.entry(name.clone()).or_insert_with(|| {
                    Histogram::<u64>::new_with_bounds(1, 60 * 60 * 1000 * 1000, 2).unwrap()
                });
                // Assuming value is compatible with u64 (like micros or simple counts)
                // If it's a float, we might need to cast.
                let _ = h.record(value as u64);
                *self.histogram_sums.entry(name).or_insert(0.0) += value;
            }
            Metric::Counter {
                name,
                value,
                tags: _,
            } => {
                *self.counters.entry(name).or_insert(0.0) += value;
            }
            Metric::Gauge {
                name,
                value,
                tags: _,
            } => {
                self.gauges.insert(name, value);
            }
            Metric::Rate {
                name,
                success,
                tags: _,
            } => {
                let entry = self.rates.entry(name).or_insert((0, 0));
                entry.0 += 1;
                if success {
                    entry.1 += 1;
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
            let (avg_blocked, avg_conn, avg_tls, avg_send, avg_wait, avg_recv, avg_resp_size) =
                if stats.total_requests > 0 {
                    (
                        stats.total_blocked.as_millis() as f64 / count,
                        stats.total_connecting.as_millis() as f64 / count,
                        stats.total_tls_handshaking.as_millis() as f64 / count,
                        stats.total_sending.as_millis() as f64 / count,
                        stats.total_waiting.as_millis() as f64 / count,
                        stats.total_receiving.as_millis() as f64 / count,
                        stats.total_response_size as f64 / count,
                    )
                } else {
                    (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
                };

            grouped_requests.insert(
                name.clone(),
                RequestReport {
                    total_requests: stats.total_requests,
                    min_latency_ms: stats.min_duration.unwrap_or_default().as_millis(),
                    max_latency_ms: stats.max_duration.as_millis(),
                    avg_latency_ms: req_avg,
                    p95_latency_ms: Duration::from_micros(stats.histogram.value_at_quantile(0.95))
                        .as_secs_f64()
                        * 1000.0,
                    p99_latency_ms: Duration::from_micros(stats.histogram.value_at_quantile(0.99))
                        .as_secs_f64()
                        * 1000.0,
                    error_count: stats.error_count,
                    avg_blocked_ms: avg_blocked,
                    avg_connecting_ms: avg_conn,
                    avg_tls_handshaking_ms: avg_tls,
                    avg_sending_ms: avg_send,
                    avg_waiting_ms: avg_wait,
                    avg_receiving_ms: avg_recv,
                    avg_response_size: avg_resp_size,
                },
            );
        }

        // Custom Metrics Reports
        let mut hist_reports = HashMap::new();
        for (name, h) in &self.histograms {
            let sum = self.histogram_sums.get(name).cloned().unwrap_or(0.0);
            let count = h.len();
            let avg = if count > 0 { sum / count as f64 } else { 0.0 };

            hist_reports.insert(
                name.clone(),
                HistogramReport {
                    avg,
                    min: h.min() as f64,
                    max: h.max() as f64,
                    p90: h.value_at_quantile(0.9) as f64,
                    p95: h.value_at_quantile(0.95) as f64,
                    p99: h.value_at_quantile(0.99) as f64,
                    count,
                },
            );
        }

        let mut rate_reports = HashMap::new();
        for (name, &(total, success)) in &self.rates {
            let rate = if total > 0 {
                success as f64 / total as f64
            } else {
                0.0
            };
            rate_reports.insert(
                name.clone(),
                RateReport {
                    total,
                    success,
                    rate,
                },
            );
        }

        ReportStats {
            total_requests: self.total_requests,
            total_duration_ms: self.total_duration.as_millis(),
            avg_latency_ms: avg_latency,
            min_latency_ms: self.min_duration.unwrap_or_default().as_millis(),
            max_latency_ms: self.max_duration.as_millis(),
            p50_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.5))
                .as_secs_f64()
                * 1000.0,
            p90_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.9))
                .as_secs_f64()
                * 1000.0,
            p95_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.95))
                .as_secs_f64()
                * 1000.0,
            p99_latency_ms: Duration::from_micros(self.histogram.value_at_quantile(0.99))
                .as_secs_f64()
                * 1000.0,
            status_codes: self.status_codes.clone(),
            errors: self.errors.clone(),
            checks: self.checks.clone(),
            grouped_requests,
            histograms: hist_reports,
            rates: rate_reports,
            counters: self.counters.clone(),
            gauges: self.gauges.clone(),
            total_data_sent: self.total_data_sent,
            total_data_received: self.total_data_received,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.to_report()).unwrap_or_default()
    }

    pub fn report(&self) {
        if self.total_requests == 0
            && self.checks.is_empty()
            && self.counters.is_empty()
            && self.gauges.is_empty()
            && self.histograms.is_empty()
            && self.rates.is_empty()
        {
            println!("\n--- Test Summary ---");
            println!("No metrics collected.");
            return;
        }

        println!("\n--- Test Summary ---");

        if self.total_requests > 0 {
            let avg_duration = self.total_duration / self.total_requests as u32;
            println!("Total Requests: {}", self.total_requests);
            println!("Avg Latency:    {:?}", avg_duration);
            println!(
                "Min Latency:    {:?}",
                self.min_duration.unwrap_or_default()
            );
            println!("Max Latency:    {:?}", self.max_duration);
            println!(
                "P50 Latency:    {:?}",
                Duration::from_micros(self.histogram.value_at_quantile(0.5))
            );
            println!(
                "P90 Latency:    {:?}",
                Duration::from_micros(self.histogram.value_at_quantile(0.9))
            );
            println!(
                "P95 Latency:    {:?}",
                Duration::from_micros(self.histogram.value_at_quantile(0.95))
            );
            println!(
                "P99 Latency:    {:?}",
                Duration::from_micros(self.histogram.value_at_quantile(0.99))
            );

            println!("\nStatus Codes:");
            let mut codes: Vec<_> = self.status_codes.iter().collect();
            codes.sort_by_key(|a| a.0);
            for (code, count) in codes {
                println!("  {}: {}", code, count);
            }

            // Print data transfer
            let mb_sent = self.total_data_sent as f64 / 1_048_576.0;
            let mb_recv = self.total_data_received as f64 / 1_048_576.0;
            println!("\nData Transfer:");
            println!("  Sent:     {:.2} MB", mb_sent);
            println!("  Received: {:.2} MB", mb_recv);
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
                    println!(
                        "  ✓ {} : {:.2}% ({} passed, {} failed)",
                        name, percent, passes, fail
                    );
                } else {
                    println!("  ✓ {} : 100% ({} passed)", name, passes);
                }
            }
        }

        // Custom Metrics Reporting
        if !self.histograms.is_empty() {
            println!("\nHistograms:");
            for (name, h) in &self.histograms {
                let p95 = h.value_at_quantile(0.95);
                let avg = self.histogram_sums.get(name).unwrap_or(&0.0) / h.len().max(1) as f64;
                println!("  {}: p95={}, avg={:.2}, count={}", name, p95, avg, h.len());
            }
        }

        if !self.counters.is_empty() {
            println!("\nCounters:");
            for (name, val) in &self.counters {
                println!("  {}: {:.2}", name, val);
            }
        }

        if !self.gauges.is_empty() {
            println!("\nGauges:");
            for (name, val) in &self.gauges {
                println!("  {}: {:.2}", name, val);
            }
        }

        if !self.rates.is_empty() {
            println!("\nRates:");
            for (name, (total, success)) in &self.rates {
                let rate = if *total > 0 {
                    *success as f64 / *total as f64
                } else {
                    0.0
                };
                println!("  {}: {:.2}% ({}/{})", name, rate * 100.0, success, total);
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
                if parts.len() < 3 {
                    eprintln!(
                        "Warning: Invalid threshold format '{}' for metric '{}'. Expected format: 'p95 < 500' (spaces required)",
                        threshold, metric_name
                    );
                    continue;
                }

                let check_type = parts[0];
                let operator = parts[1];
                let value: f64 = parts[2].parse().unwrap_or(0.0);

                let actual_value = if metric_name == "http_req_duration" {
                    match check_type {
                        "p95" => report.p95_latency_ms,
                        "p99" => report.p99_latency_ms,
                        "avg" => report.avg_latency_ms,
                        "max" => report.max_latency_ms as f64,
                        "min" => report.min_latency_ms as f64,
                        _ => 0.0,
                    }
                } else if metric_name == "http_req_failed" {
                    if check_type == "rate" {
                        if report.total_requests > 0 {
                            let total_errors: usize = report.errors.values().sum::<usize>()
                                + report
                                    .grouped_requests
                                    .values()
                                    .map(|r| r.error_count)
                                    .sum::<usize>();
                            total_errors as f64 / report.total_requests as f64
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    }
                } else if let Some(h) = report.histograms.get(metric_name) {
                    // Custom Histogram
                    match check_type {
                        "p95" => h.p95,
                        "p99" => h.p99,
                        "p90" => h.p90,
                        "avg" => h.avg,
                        "max" => h.max,
                        "min" => h.min,
                        "count" => h.count as f64,
                        _ => 0.0,
                    }
                } else if let Some(r) = report.rates.get(metric_name) {
                    // Custom Rate
                    if check_type == "rate" {
                        r.rate
                    } else {
                        0.0
                    }
                } else if let Some(val) = report.counters.get(metric_name) {
                    // Custom Counter
                    if check_type == "count" || check_type == "value" {
                        *val
                    } else {
                        0.0
                    }
                } else if let Some(val) = report.gauges.get(metric_name) {
                    // Custom Gauge
                    if check_type == "value" {
                        *val
                    } else {
                        0.0
                    }
                } else {
                    0.0
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
                    failures.push(format!(
                        "Threshold FAILED: {} {} {} (actual: {:.2})",
                        metric_name, check_type, threshold, actual_value
                    ));
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
            tags: HashMap::new(),
        });

        agg.add(Metric::Request {
            name: "test".to_string(),
            timings: RequestTimings {
                duration: Duration::from_millis(200),
                ..Default::default()
            },
            status: 200,
            error: None,
            tags: HashMap::new(),
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
            tags: HashMap::new(),
        });

        assert_eq!(agg.total_requests, 1);
        assert_eq!(*agg.errors.get("Timeout").unwrap(), 1);
    }

    #[test]
    fn test_aggregator_checks() {
        let mut agg = StatsAggregator::new();

        agg.add(Metric::Check {
            name: "status is 200".to_string(),
            success: true,
        });
        agg.add(Metric::Check {
            name: "status is 200".to_string(),
            success: false,
        });

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
                tags: HashMap::new(),
            });
        }

        // P50 should be around 50ms (50000us)
        let p50 = agg.histogram.value_at_quantile(0.5);
        assert!((49000..=51000).contains(&p50), "P50 was {}", p50);

        // P99 should be around 99ms (99000us)
        let p99 = agg.histogram.value_at_quantile(0.99);
        assert!((98000..=100000).contains(&p99), "P99 was {}", p99);
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
            tags: HashMap::new(),
        });

        let report = agg.to_report();
        assert_eq!(report.total_requests, 1);
        assert_eq!(report.avg_latency_ms, 100.0);

        let json = agg.to_json();
        assert!(json.contains("\"total_requests\": 1"));
        assert!(json.contains("\"avg_latency_ms\": 100.0"));

        let html = crate::stats::html::generate_html(&report);
        assert!(html.contains("Fusillade Load Test Report"));
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
                tags: HashMap::new(),
            });
        }

        let mut criteria = HashMap::new();
        // Should pass: p95 is ~95ms, which is < 200
        criteria.insert(
            "http_req_duration".to_string(),
            vec!["p95 < 200".to_string()],
        );
        // Should pass: rate is 0, which is < 0.1
        criteria.insert(
            "http_req_failed".to_string(),
            vec!["rate < 0.1".to_string()],
        );

        let failures = agg.validate_thresholds(&criteria);
        assert!(
            failures.is_empty(),
            "Expected no failures, got: {:?}",
            failures
        );

        // Test failing criteria
        let mut fail_criteria = HashMap::new();
        fail_criteria.insert(
            "http_req_duration".to_string(),
            vec!["p95 < 50".to_string()],
        );

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
            tags: HashMap::new(),
        });

        let mut c = HashMap::new();
        c.insert(
            "http_req_duration".to_string(),
            vec!["avg <= 100".to_string()],
        );
        assert!(agg.validate_thresholds(&c).is_empty());

        let mut c = HashMap::new();
        c.insert(
            "http_req_duration".to_string(),
            vec!["avg >= 100".to_string()],
        );
        assert!(agg.validate_thresholds(&c).is_empty());

        let mut c = HashMap::new();
        c.insert(
            "http_req_duration".to_string(),
            vec!["avg > 100".to_string()],
        );
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
        assert!(html.contains("No requests were made"));
    }

    // ==================== Custom Metrics Tests ====================

    #[test]
    fn test_custom_histogram() {
        let mut agg = StatsAggregator::new();

        // Add values to a custom histogram
        for i in 1..=100 {
            agg.add(Metric::Histogram {
                name: "checkout_duration".to_string(),
                value: i as f64,
                tags: HashMap::new(),
            });
        }

        let report = agg.to_report();
        let hist = report
            .histograms
            .get("checkout_duration")
            .expect("Histogram should exist");

        assert_eq!(hist.count, 100);
        assert_eq!(hist.min, 1.0);
        assert_eq!(hist.max, 100.0);
        assert!((hist.avg - 50.5).abs() < 1.0, "Avg was {}", hist.avg);
        assert!(hist.p95 >= 94.0 && hist.p95 <= 96.0, "P95 was {}", hist.p95);
    }

    #[test]
    fn test_custom_counter() {
        let mut agg = StatsAggregator::new();

        // Add values to a counter
        agg.add(Metric::Counter {
            name: "items_sold".to_string(),
            value: 5.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Counter {
            name: "items_sold".to_string(),
            value: 3.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Counter {
            name: "items_sold".to_string(),
            value: 2.0,
            tags: HashMap::new(),
        });

        let report = agg.to_report();
        let counter = report
            .counters
            .get("items_sold")
            .expect("Counter should exist");

        assert_eq!(*counter, 10.0);
    }

    #[test]
    fn test_custom_gauge() {
        let mut agg = StatsAggregator::new();

        // Gauge should always store the latest value
        agg.add(Metric::Gauge {
            name: "queue_size".to_string(),
            value: 10.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Gauge {
            name: "queue_size".to_string(),
            value: 25.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Gauge {
            name: "queue_size".to_string(),
            value: 5.0,
            tags: HashMap::new(),
        });

        let report = agg.to_report();
        let gauge = report.gauges.get("queue_size").expect("Gauge should exist");

        assert_eq!(*gauge, 5.0); // Should be the last value
    }

    #[test]
    fn test_custom_rate() {
        let mut agg = StatsAggregator::new();

        // Add some successes and failures
        for _ in 0..8 {
            agg.add(Metric::Rate {
                name: "cache_hit".to_string(),
                success: true,
                tags: HashMap::new(),
            });
        }
        for _ in 0..2 {
            agg.add(Metric::Rate {
                name: "cache_hit".to_string(),
                success: false,
                tags: HashMap::new(),
            });
        }

        let report = agg.to_report();
        let rate = report.rates.get("cache_hit").expect("Rate should exist");

        assert_eq!(rate.total, 10);
        assert_eq!(rate.success, 8);
        assert!(
            (rate.rate - 0.8).abs() < f64::EPSILON,
            "Rate was {}",
            rate.rate
        );
    }

    #[test]
    fn test_custom_metrics_threshold_validation() {
        let mut agg = StatsAggregator::new();

        // Add custom histogram values
        for i in 1..=100 {
            agg.add(Metric::Histogram {
                name: "api_latency".to_string(),
                value: i as f64,
                tags: HashMap::new(),
            });
        }

        // Add custom counter
        agg.add(Metric::Counter {
            name: "requests".to_string(),
            value: 150.0,
            tags: HashMap::new(),
        });

        // Add custom rate (90% success)
        for _ in 0..9 {
            agg.add(Metric::Rate {
                name: "success".to_string(),
                success: true,
                tags: HashMap::new(),
            });
        }
        agg.add(Metric::Rate {
            name: "success".to_string(),
            success: false,
            tags: HashMap::new(),
        });

        // Add custom gauge
        agg.add(Metric::Gauge {
            name: "memory".to_string(),
            value: 50.0,
            tags: HashMap::new(),
        });

        // Test passing thresholds
        let mut criteria = HashMap::new();
        criteria.insert("api_latency".to_string(), vec!["p95 < 200".to_string()]);
        criteria.insert("requests".to_string(), vec!["count > 100".to_string()]);
        criteria.insert("success".to_string(), vec!["rate > 0.8".to_string()]);
        criteria.insert("memory".to_string(), vec!["value < 100".to_string()]);

        let failures = agg.validate_thresholds(&criteria);
        assert!(
            failures.is_empty(),
            "Expected no failures, got: {:?}",
            failures
        );

        // Test failing thresholds
        let mut fail_criteria = HashMap::new();
        fail_criteria.insert("api_latency".to_string(), vec!["p95 < 50".to_string()]);
        fail_criteria.insert("requests".to_string(), vec!["count > 200".to_string()]);
        fail_criteria.insert("success".to_string(), vec!["rate > 0.95".to_string()]);
        fail_criteria.insert("memory".to_string(), vec!["value < 25".to_string()]);

        let failures = agg.validate_thresholds(&fail_criteria);
        assert_eq!(
            failures.len(),
            4,
            "Expected 4 failures, got: {:?}",
            failures
        );
    }

    #[test]
    fn test_custom_metrics_in_json() {
        let mut agg = StatsAggregator::new();

        agg.add(Metric::Histogram {
            name: "latency".to_string(),
            value: 100.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Counter {
            name: "count".to_string(),
            value: 42.0,
            tags: HashMap::new(),
        });
        agg.add(Metric::Gauge {
            name: "temp".to_string(),
            value: 98.6,
            tags: HashMap::new(),
        });
        agg.add(Metric::Rate {
            name: "hit_rate".to_string(),
            success: true,
            tags: HashMap::new(),
        });

        let json = agg.to_json();

        assert!(
            json.contains("\"histograms\""),
            "JSON should contain histograms"
        );
        assert!(
            json.contains("\"counters\""),
            "JSON should contain counters"
        );
        assert!(json.contains("\"gauges\""), "JSON should contain gauges");
        assert!(json.contains("\"rates\""), "JSON should contain rates");
        assert!(
            json.contains("\"latency\""),
            "JSON should contain latency histogram"
        );
        assert!(
            json.contains("\"count\""),
            "JSON should contain count counter"
        );
        assert!(json.contains("\"temp\""), "JSON should contain temp gauge");
        assert!(
            json.contains("\"hit_rate\""),
            "JSON should contain hit_rate rate"
        );
    }
}
