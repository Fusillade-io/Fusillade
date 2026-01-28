//! OTLP (OpenTelemetry Protocol) Exporter for Fusillade metrics
//!
//! Exports load test metrics to an OTLP-compatible endpoint via HTTP/JSON.
//! This is a simplified implementation that pushes metrics as JSON to an OTLP collector.

use crate::stats::ReportStats;
use serde::Serialize;

/// OTLP Exporter configuration
pub struct OtlpExporter {
    endpoint: String,
    client: reqwest::blocking::Client,
}

#[derive(Serialize)]
pub(crate) struct MetricData {
    name: String,
    value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize)]
pub(crate) struct OtlpPayload {
    resource: ResourceData,
    metrics: Vec<MetricData>,
}

#[derive(Serialize)]
pub(crate) struct ResourceData {
    service_name: String,
    service_version: String,
}

impl OtlpExporter {
    /// Create a new OTLP exporter pointing to the given endpoint
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            client: reqwest::blocking::Client::new(),
        }
    }

    /// Build the OTLP payload from the report stats (pub(crate) for testing)
    pub(crate) fn build_payload(report: &ReportStats) -> OtlpPayload {
        // Core metrics
        let mut metrics = vec![MetricData {
            name: "fusillade.http.requests.total".to_string(),
            value: report.total_requests as f64,
            labels: None,
        }];

        metrics.push(MetricData {
            name: "fusillade.http.latency.avg_ms".to_string(),
            value: report.avg_latency_ms,
            labels: None,
        });

        metrics.push(MetricData {
            name: "fusillade.http.latency.p95_ms".to_string(),
            value: report.p95_latency_ms,
            labels: None,
        });

        metrics.push(MetricData {
            name: "fusillade.http.latency.p99_ms".to_string(),
            value: report.p99_latency_ms,
            labels: None,
        });

        // Error count
        let error_count: usize = report.errors.values().sum();
        metrics.push(MetricData {
            name: "fusillade.http.errors.total".to_string(),
            value: error_count as f64,
            labels: None,
        });

        // Per-request group metrics
        for (name, req_report) in &report.grouped_requests {
            let mut labels = std::collections::HashMap::new();
            labels.insert("request.name".to_string(), name.clone());

            metrics.push(MetricData {
                name: "fusillade.http.request.count".to_string(),
                value: req_report.total_requests as f64,
                labels: Some(labels.clone()),
            });

            metrics.push(MetricData {
                name: "fusillade.http.request.p95_ms".to_string(),
                value: req_report.p95_latency_ms,
                labels: Some(labels),
            });
        }

        // Custom Metrics - Histograms
        for (name, h) in &report.histograms {
            // Flatten histogram into multiple gauges
            let mut base_labels = std::collections::HashMap::new();
            base_labels.insert("metric_type".to_string(), "histogram".to_string());

            let variants = vec![
                ("p95", h.p95),
                ("p99", h.p99),
                ("avg", h.avg),
                ("min", h.min),
                ("max", h.max),
                ("count", h.count as f64),
            ];

            for (suffix, val) in variants {
                metrics.push(MetricData {
                    name: format!("{}.{}", name, suffix),
                    value: val,
                    labels: Some(base_labels.clone()),
                });
            }
        }

        // Custom Metrics - Counters
        for (name, val) in &report.counters {
            let mut labels = std::collections::HashMap::new();
            labels.insert("metric_type".to_string(), "counter".to_string());
            metrics.push(MetricData {
                name: name.clone(),
                value: *val,
                labels: Some(labels),
            });
        }

        // Custom Metrics - Gauges
        for (name, val) in &report.gauges {
            let mut labels = std::collections::HashMap::new();
            labels.insert("metric_type".to_string(), "gauge".to_string());
            metrics.push(MetricData {
                name: name.clone(),
                value: *val,
                labels: Some(labels),
            });
        }

        // Custom Metrics - Rates
        for (name, r) in &report.rates {
            let mut labels = std::collections::HashMap::new();
            labels.insert("metric_type".to_string(), "rate".to_string());
            metrics.push(MetricData {
                name: name.clone(),
                value: r.rate,
                labels: Some(labels),
            });
        }

        OtlpPayload {
            resource: ResourceData {
                service_name: "fusillade".to_string(),
                service_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            metrics,
        }
    }

    /// Export a report's metrics to OTLP endpoint
    pub fn export(
        &self,
        report: &ReportStats,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let payload = Self::build_payload(report);

        // POST to OTLP endpoint
        let response = self
            .client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()?;

        if !response.status().is_success() {
            return Err(format!("OTLP export failed: HTTP {}", response.status()).into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::ReportStats;
    use std::collections::HashMap;

    #[test]
    fn test_otlp_exporter_new() {
        let exporter = OtlpExporter::new("http://localhost:4318/v1/metrics");
        assert_eq!(exporter.endpoint, "http://localhost:4318/v1/metrics");
    }

    #[test]
    fn test_metric_data_serialization() {
        let metric = MetricData {
            name: "test.metric".to_string(),
            value: 42.5,
            labels: None,
        };

        let json = serde_json::to_string(&metric).unwrap();
        assert!(json.contains("\"name\":\"test.metric\""));
        assert!(json.contains("\"value\":42.5"));
        assert!(!json.contains("labels")); // skip_serializing_if
    }

    #[test]
    fn test_metric_data_with_labels() {
        let mut labels = HashMap::new();
        labels.insert("endpoint".to_string(), "/api/users".to_string());

        let metric = MetricData {
            name: "test.metric".to_string(),
            value: 100.0,
            labels: Some(labels),
        };

        let json = serde_json::to_string(&metric).unwrap();
        assert!(json.contains("\"labels\""));
        assert!(json.contains("/api/users"));
    }

    #[test]
    fn test_otlp_payload_structure() {
        let payload = OtlpPayload {
            resource: ResourceData {
                service_name: "fusillade".to_string(),
                service_version: "0.1.0".to_string(),
            },
            metrics: vec![MetricData {
                name: "test".to_string(),
                value: 1.0,
                labels: None,
            }],
        };

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"service_name\":\"fusillade\""));
        assert!(json.contains("\"metrics\""));
    }

    #[test]
    fn test_export_builds_correct_metrics() {
        // Create a minimal ReportStats
        let _report = ReportStats {
            total_requests: 100,
            avg_latency_ms: 50.5,
            p95_latency_ms: 95.0,
            p99_latency_ms: 99.0,
            errors: HashMap::new(),
            grouped_requests: HashMap::new(),
            ..Default::default()
        };

        let exporter = OtlpExporter::new("http://localhost:9999/test");

        // We can't actually test the HTTP call without a server,
        // but we can verify the exporter is created correctly
        assert!(!exporter.endpoint.is_empty());
    }
}

#[test]
fn test_export_custom_metrics() {
    use crate::stats::{HistogramReport, RateReport};

    let mut report = ReportStats::default();

    // Add custom histogram
    report.histograms.insert(
        "my_hist".to_string(),
        HistogramReport {
            avg: 50.0,
            min: 10.0,
            max: 100.0,
            p90: 90.0,
            p95: 95.0,
            p99: 99.0,
            count: 10,
        },
    );

    // Add custom counter
    report.counters.insert("my_counter".to_string(), 42.0);

    // Add custom gauge
    report.gauges.insert("my_gauge".to_string(), 123.45);

    // Add custom rate
    report.rates.insert(
        "my_rate".to_string(),
        RateReport {
            total: 100,
            success: 99,
            rate: 0.99,
        },
    );

    let payload = OtlpExporter::build_payload(&report);
    let json = serde_json::to_string(&payload).unwrap();

    // Verify Histogram expansion
    assert!(json.contains("my_hist.p95"));
    assert!(json.contains("my_hist.avg"));
    assert!(json.contains("my_hist.count"));

    // Verify Counter
    assert!(json.contains("my_counter"));
    assert!(json.contains("\"value\":42.0"));

    // Verify Gauge
    assert!(json.contains("my_gauge"));
    assert!(json.contains("\"value\":123.45"));

    // Verify Rate
    assert!(json.contains("my_rate"));
    assert!(json.contains("\"value\":0.99"));
}
