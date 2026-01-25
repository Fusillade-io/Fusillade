use crate::stats::ReportStats;

/// Generate CSV output from ReportStats
pub fn generate_csv(report: &ReportStats) -> String {
    let mut out = String::from("metric_name,metric_type,value\n");

    // Global request metrics
    out.push_str(&format!(
        "http_req_count,counter,{}\n",
        report.total_requests
    ));
    out.push_str(&format!(
        "http_req_duration_avg,gauge,{:.3}\n",
        report.avg_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_min,gauge,{}\n",
        report.min_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_max,gauge,{}\n",
        report.max_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_p50,gauge,{:.3}\n",
        report.p50_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_p90,gauge,{:.3}\n",
        report.p90_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_p95,gauge,{:.3}\n",
        report.p95_latency_ms
    ));
    out.push_str(&format!(
        "http_req_duration_p99,gauge,{:.3}\n",
        report.p99_latency_ms
    ));

    // Status codes
    for (code, count) in &report.status_codes {
        out.push_str(&format!("http_req_status_{},counter,{}\n", code, count));
    }

    // Checks (tuple: passed, failed)
    for (name, (passed, failed)) in &report.checks {
        let safe_name = name.replace([' ', ':'], "_");
        out.push_str(&format!("check_{}_passed,counter,{}\n", safe_name, passed));
        out.push_str(&format!("check_{}_failed,counter,{}\n", safe_name, failed));
    }

    // Custom histograms
    for (name, hist) in &report.histograms {
        out.push_str(&format!("{}_count,counter,{}\n", name, hist.count));
        out.push_str(&format!("{}_avg,gauge,{:.3}\n", name, hist.avg));
        out.push_str(&format!("{}_p95,gauge,{:.3}\n", name, hist.p95));
    }

    // Custom counters
    for (name, value) in &report.counters {
        out.push_str(&format!("{},counter,{}\n", name, value));
    }

    // Custom rates
    for (name, rate) in &report.rates {
        out.push_str(&format!("{}_rate,gauge,{:.4}\n", name, rate.rate));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_csv_generation() {
        let report = ReportStats {
            total_requests: 100,
            avg_latency_ms: 45.5,
            min_latency_ms: 10,
            max_latency_ms: 200,
            p50_latency_ms: 40.0,
            p90_latency_ms: 80.0,
            p95_latency_ms: 100.0,
            p99_latency_ms: 150.0,
            status_codes: HashMap::from([(200, 95), (500, 5)]),
            ..Default::default()
        };

        let csv = generate_csv(&report);
        assert!(csv.contains("http_req_count,counter,100"));
        assert!(csv.contains("http_req_duration_avg,gauge,45.500"));
        assert!(csv.contains("http_req_status_200,counter,95"));
    }
}
