use crate::stats::ReportStats;

/// Escape a value for CSV output per RFC 4180.
/// Wraps in double quotes and doubles internal quotes if the value
/// contains commas, double quotes, or newlines.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

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

    // Checks (tuple: total, passes)
    for (name, (total, passes)) in &report.checks {
        let safe_name = csv_escape(&name.replace([' ', ':'], "_"));
        out.push_str(&format!("check_{}_passed,counter,{}\n", safe_name, passes));
        out.push_str(&format!(
            "check_{}_failed,counter,{}\n",
            safe_name,
            total - passes
        ));
    }

    // Custom histograms
    for (name, hist) in &report.histograms {
        let safe = csv_escape(name);
        out.push_str(&format!("{}_count,counter,{}\n", safe, hist.count));
        out.push_str(&format!("{}_avg,gauge,{:.3}\n", safe, hist.avg));
        out.push_str(&format!("{}_p95,gauge,{:.3}\n", safe, hist.p95));
    }

    // Custom counters
    for (name, value) in &report.counters {
        out.push_str(&format!("{},counter,{}\n", csv_escape(name), value));
    }

    // Custom rates
    for (name, rate) in &report.rates {
        out.push_str(&format!(
            "{}_rate,gauge,{:.4}\n",
            csv_escape(name),
            rate.rate
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_csv_checks_tuple_correctness() {
        // Verify checks use (total, passes) not (passed, failed)
        let report = ReportStats {
            checks: HashMap::from([("status is 200".to_string(), (100, 95))]),
            ..Default::default()
        };

        let csv = generate_csv(&report);
        assert!(csv.contains("check_status_is_200_passed,counter,95"));
        assert!(csv.contains("check_status_is_200_failed,counter,5"));
    }

    #[test]
    fn test_csv_checks_all_passed() {
        let report = ReportStats {
            checks: HashMap::from([("all pass".to_string(), (50, 50))]),
            ..Default::default()
        };

        let csv = generate_csv(&report);
        assert!(csv.contains("check_all_pass_passed,counter,50"));
        assert!(csv.contains("check_all_pass_failed,counter,0"));
    }

    #[test]
    fn test_csv_empty_report() {
        let report = ReportStats::default();
        let csv = generate_csv(&report);
        assert!(csv.contains("http_req_count,counter,0"));
    }

    #[test]
    fn test_csv_escape_comma() {
        assert_eq!(csv_escape("name,with,commas"), "\"name,with,commas\"");
    }

    #[test]
    fn test_csv_escape_quotes() {
        assert_eq!(csv_escape("say \"hello\""), "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn test_csv_escape_newline() {
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn test_csv_escape_carriage_return() {
        assert_eq!(csv_escape("line1\rline2"), "\"line1\rline2\"");
    }

    #[test]
    fn test_csv_escape_clean() {
        assert_eq!(csv_escape("simple_name"), "simple_name");
    }

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
