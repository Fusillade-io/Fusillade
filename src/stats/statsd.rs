//! StatsD Exporter for Fusillade metrics
//!
//! Exports load test metrics to a StatsD-compatible endpoint via UDP.
//! Compatible with StatsD, Datadog, Graphite, and other StatsD receivers.
//!
//! Format: `metric.name:value|type|#tag1:value1,tag2:value2`
//! Types: g (gauge), c (counter), ms (timing)

use crate::stats::ReportStats;
use std::net::UdpSocket;

/// StatsD Exporter configuration
pub struct StatsdExporter {
    endpoint: String,
    prefix: String,
}

impl StatsdExporter {
    /// Create a new StatsD exporter pointing to the given endpoint (host:port)
    /// Default port is 8125 if not specified
    pub fn new(endpoint: &str) -> Self {
        let endpoint = if endpoint.contains(':') {
            endpoint.to_string()
        } else {
            format!("{}:8125", endpoint)
        };

        Self {
            endpoint,
            prefix: "fusillade".to_string(),
        }
    }

    /// Create a new StatsD exporter with a custom metric prefix
    pub fn with_prefix(endpoint: &str, prefix: &str) -> Self {
        let mut exporter = Self::new(endpoint);
        exporter.prefix = prefix.to_string();
        exporter
    }

    /// Build StatsD metric lines from the report stats
    pub(crate) fn build_metrics(&self, report: &ReportStats) -> Vec<String> {
        let mut metrics = Vec::new();

        // Core metrics
        metrics.push(format!(
            "{}.http.requests.total:{}|c",
            self.prefix, report.total_requests
        ));

        // Calculate RPS from total_requests / total_duration
        let rps = if report.total_duration_ms > 0 {
            report.total_requests as f64 / (report.total_duration_ms as f64 / 1000.0)
        } else {
            0.0
        };
        metrics.push(format!("{}.http.rps:{:.2}|g", self.prefix, rps));

        metrics.push(format!(
            "{}.http.latency.avg:{:.2}|ms",
            self.prefix, report.avg_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.min:{}|ms",
            self.prefix, report.min_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.max:{}|ms",
            self.prefix, report.max_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.p50:{:.2}|ms",
            self.prefix, report.p50_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.p90:{:.2}|ms",
            self.prefix, report.p90_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.p95:{:.2}|ms",
            self.prefix, report.p95_latency_ms
        ));

        metrics.push(format!(
            "{}.http.latency.p99:{:.2}|ms",
            self.prefix, report.p99_latency_ms
        ));

        // Error count
        let error_count: usize = report.errors.values().sum();
        metrics.push(format!(
            "{}.http.errors.total:{}|c",
            self.prefix, error_count
        ));

        // Per-error type metrics
        for (error_type, count) in &report.errors {
            let safe_error = sanitize_metric_name(error_type);
            metrics.push(format!(
                "{}.http.errors.by_type:{}|c|#error:{}",
                self.prefix, count, safe_error
            ));
        }

        // HTTP status code metrics
        for (status, count) in &report.status_codes {
            metrics.push(format!(
                "{}.http.status:{}|c|#code:{}",
                self.prefix, count, status
            ));
        }

        // Per-request group metrics (endpoints)
        for (name, req_report) in &report.grouped_requests {
            let safe_name = sanitize_metric_name(name);

            metrics.push(format!(
                "{}.http.endpoint.requests:{}|c|#endpoint:{}",
                self.prefix, req_report.total_requests, safe_name
            ));

            metrics.push(format!(
                "{}.http.endpoint.latency.avg:{:.2}|ms|#endpoint:{}",
                self.prefix, req_report.avg_latency_ms, safe_name
            ));

            metrics.push(format!(
                "{}.http.endpoint.latency.p95:{:.2}|ms|#endpoint:{}",
                self.prefix, req_report.p95_latency_ms, safe_name
            ));

            metrics.push(format!(
                "{}.http.endpoint.latency.p99:{:.2}|ms|#endpoint:{}",
                self.prefix, req_report.p99_latency_ms, safe_name
            ));

            metrics.push(format!(
                "{}.http.endpoint.errors:{}|c|#endpoint:{}",
                self.prefix, req_report.error_count, safe_name
            ));
        }

        // Custom Histograms (HistogramReport structure)
        for (name, h) in &report.histograms {
            let safe_name = sanitize_metric_name(name);

            metrics.push(format!(
                "{}.custom.histogram.count:{}|c|#name:{}",
                self.prefix, h.count, safe_name
            ));

            metrics.push(format!(
                "{}.custom.histogram.min:{:.2}|g|#name:{}",
                self.prefix, h.min, safe_name
            ));

            metrics.push(format!(
                "{}.custom.histogram.max:{:.2}|g|#name:{}",
                self.prefix, h.max, safe_name
            ));

            metrics.push(format!(
                "{}.custom.histogram.avg:{:.2}|g|#name:{}",
                self.prefix, h.avg, safe_name
            ));

            metrics.push(format!(
                "{}.custom.histogram.p95:{:.2}|g|#name:{}",
                self.prefix, h.p95, safe_name
            ));
        }

        // Custom Counters
        for (name, value) in &report.counters {
            let safe_name = sanitize_metric_name(name);
            metrics.push(format!(
                "{}.custom.counter:{}|c|#name:{}",
                self.prefix, value, safe_name
            ));
        }

        // Custom Gauges
        for (name, value) in &report.gauges {
            let safe_name = sanitize_metric_name(name);
            metrics.push(format!(
                "{}.custom.gauge:{}|g|#name:{}",
                self.prefix, value, safe_name
            ));
        }

        // Custom Rates (RateReport structure)
        for (name, rate_report) in &report.rates {
            let safe_name = sanitize_metric_name(name);
            metrics.push(format!(
                "{}.custom.rate:{:.4}|g|#name:{}",
                self.prefix, rate_report.rate, safe_name
            ));
            metrics.push(format!(
                "{}.custom.rate.successes:{}|c|#name:{}",
                self.prefix, rate_report.success, safe_name
            ));
            metrics.push(format!(
                "{}.custom.rate.total:{}|c|#name:{}",
                self.prefix, rate_report.total, safe_name
            ));
        }

        // Data transfer metrics
        metrics.push(format!(
            "{}.http.bytes.sent:{}|c",
            self.prefix, report.total_data_sent
        ));

        metrics.push(format!(
            "{}.http.bytes.received:{}|c",
            self.prefix, report.total_data_received
        ));

        // Checks metrics
        let (checks_passed, checks_failed): (usize, usize) = report
            .checks
            .values()
            .fold((0, 0), |(p, f), (passed, failed)| (p + passed, f + failed));

        if checks_passed > 0 || checks_failed > 0 {
            metrics.push(format!("{}.checks.passed:{}|c", self.prefix, checks_passed));
            metrics.push(format!("{}.checks.failed:{}|c", self.prefix, checks_failed));
        }

        metrics
    }

    /// Export metrics to the StatsD endpoint via UDP
    pub fn export(&self, report: &ReportStats) -> Result<(), String> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .map_err(|e| format!("Failed to bind UDP socket: {}", e))?;

        let metrics = self.build_metrics(report);

        // StatsD typically has a max packet size of ~1432 bytes (MTU - headers)
        // We batch multiple metrics per packet, separated by newlines
        const MAX_PACKET_SIZE: usize = 1400;

        let mut batch = String::new();
        let mut sent_count = 0;

        for metric in &metrics {
            // If adding this metric would exceed max size, send current batch
            if !batch.is_empty() && batch.len() + metric.len() + 1 > MAX_PACKET_SIZE {
                socket
                    .send_to(batch.as_bytes(), &self.endpoint)
                    .map_err(|e| format!("Failed to send UDP packet: {}", e))?;
                sent_count += 1;
                batch.clear();
            }

            if !batch.is_empty() {
                batch.push('\n');
            }
            batch.push_str(metric);
        }

        // Send remaining metrics
        if !batch.is_empty() {
            socket
                .send_to(batch.as_bytes(), &self.endpoint)
                .map_err(|e| format!("Failed to send UDP packet: {}", e))?;
            sent_count += 1;
        }

        println!(
            "Sent {} metrics in {} UDP packets to {}",
            metrics.len(),
            sent_count,
            self.endpoint
        );

        Ok(())
    }
}

/// Sanitize a metric name for StatsD compatibility
/// Replaces invalid characters with underscores
fn sanitize_metric_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{HistogramReport, RateReport, ReportStats};
    use std::collections::HashMap;

    fn create_test_report() -> ReportStats {
        let mut report = ReportStats {
            total_requests: 1000,
            total_duration_ms: 20000, // 20 seconds -> 50 RPS
            avg_latency_ms: 25.0,
            min_latency_ms: 5,
            max_latency_ms: 150,
            p50_latency_ms: 20.0,
            p90_latency_ms: 45.0,
            p95_latency_ms: 60.0,
            p99_latency_ms: 100.0,
            errors: HashMap::new(),
            status_codes: HashMap::new(),
            grouped_requests: HashMap::new(),
            histograms: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            rates: HashMap::new(),
            total_data_sent: 50000,
            total_data_received: 250000,
            checks: HashMap::new(),
        };

        report.errors.insert("timeout".to_string(), 10);
        report.status_codes.insert(200, 950);
        report.status_codes.insert(500, 50);
        report.counters.insert("my_counter".to_string(), 42.0);
        report.gauges.insert("my_gauge".to_string(), 3.15);
        report.rates.insert(
            "success_rate".to_string(),
            RateReport {
                total: 100,
                success: 90,
                rate: 0.9,
            },
        );
        report.checks.insert("status_ok".to_string(), (95, 5));

        report
    }

    #[test]
    fn test_statsd_exporter_new() {
        let exporter = StatsdExporter::new("localhost");
        assert_eq!(exporter.endpoint, "localhost:8125");
        assert_eq!(exporter.prefix, "fusillade");

        let exporter = StatsdExporter::new("localhost:9999");
        assert_eq!(exporter.endpoint, "localhost:9999");
    }

    #[test]
    fn test_statsd_exporter_with_prefix() {
        let exporter = StatsdExporter::with_prefix("localhost", "myapp");
        assert_eq!(exporter.prefix, "myapp");
    }

    #[test]
    fn test_build_metrics_core() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        // Check core metrics exist
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.requests.total:1000|c")));
        assert!(metrics.iter().any(|m| m.contains("fusillade.http.rps:50")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.latency.avg:25")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.latency.p95:60")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.latency.p99:100")));
    }

    #[test]
    fn test_build_metrics_errors() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.errors.total:10|c")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.errors.by_type:10|c|#error:timeout")));
    }

    #[test]
    fn test_build_metrics_status_codes() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.status:950|c|#code:200")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.status:50|c|#code:500")));
    }

    #[test]
    fn test_build_metrics_custom() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.custom.counter:42|c|#name:my_counter")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.custom.gauge:3.15|g|#name:my_gauge")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.custom.rate:0.9")));
    }

    #[test]
    fn test_build_metrics_data_transfer() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.bytes.sent:50000|c")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.http.bytes.received:250000|c")));
    }

    #[test]
    fn test_build_metrics_checks() {
        let report = create_test_report();
        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.checks.passed:95|c")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.checks.failed:5|c")));
    }

    #[test]
    fn test_sanitize_metric_name() {
        assert_eq!(sanitize_metric_name("hello_world"), "hello_world");
        assert_eq!(sanitize_metric_name("GET /api/users"), "GET__api_users");
        assert_eq!(sanitize_metric_name("test:metric"), "test_metric");
        assert_eq!(sanitize_metric_name("valid.name-123"), "valid.name-123");
    }

    #[test]
    fn test_custom_prefix() {
        let report = create_test_report();
        let exporter = StatsdExporter::with_prefix("localhost", "myapp.loadtest");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics.iter().any(|m| m.starts_with("myapp.loadtest.http")));
        assert!(!metrics.iter().any(|m| m.starts_with("fusillade.")));
    }

    #[test]
    fn test_histogram_metrics() {
        let mut report = create_test_report();
        report.histograms.insert(
            "response_size".to_string(),
            HistogramReport {
                avg: 1024.5,
                min: 100.0,
                max: 5000.0,
                p90: 2000.0,
                p95: 3000.0,
                p99: 4500.0,
                count: 500,
            },
        );

        let exporter = StatsdExporter::new("localhost");
        let metrics = exporter.build_metrics(&report);

        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.custom.histogram.count:500|c|#name:response_size")));
        assert!(metrics
            .iter()
            .any(|m| m.contains("fusillade.custom.histogram.avg:1024.5")));
    }
}
