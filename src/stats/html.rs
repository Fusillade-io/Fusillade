use crate::stats::ReportStats;

/// Generate a styled HTML report from test results
pub fn generate_html(report: &ReportStats) -> String {
    if report.total_requests == 0 {
        return r#"<!DOCTYPE html>
<html><head><title>Fusillade Report</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 800px; margin: 0 auto; padding: 20px;">
<h1>Fusillade Load Test Report</h1>
<p>No requests were made during this test.</p>
</body></html>"#
            .to_string();
    }

    let mut html = String::new();

    // Header and styles
    html.push_str(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Fusillade Load Test Report</title>
    <style>
        * { box-sizing: border-box; }
        body {
            font-family: system-ui, -apple-system, sans-serif;
            max-width: 1200px;
            margin: 0 auto;
            padding: 20px;
            background: #f5f5f5;
            color: #333;
        }
        h1 { color: #1a1a2e; border-bottom: 3px solid #e94560; padding-bottom: 10px; }
        h2 { color: #16213e; margin-top: 30px; }
        .summary-grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
            gap: 15px;
            margin: 20px 0;
        }
        .stat-card {
            background: white;
            border-radius: 8px;
            padding: 20px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }
        .stat-card .label { color: #666; font-size: 14px; margin-bottom: 5px; }
        .stat-card .value { font-size: 28px; font-weight: bold; color: #1a1a2e; }
        .stat-card .unit { font-size: 14px; color: #888; }
        .stat-card.success .value { color: #28a745; }
        .stat-card.warning .value { color: #ffc107; }
        .stat-card.error .value { color: #dc3545; }
        table {
            width: 100%;
            border-collapse: collapse;
            background: white;
            border-radius: 8px;
            overflow: hidden;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
            margin: 15px 0;
        }
        th, td { padding: 12px 15px; text-align: left; border-bottom: 1px solid #eee; }
        th { background: #1a1a2e; color: white; font-weight: 500; }
        tr:hover { background: #f8f9fa; }
        .bar-container { width: 100px; height: 8px; background: #eee; border-radius: 4px; display: inline-block; }
        .bar { height: 100%; background: #e94560; border-radius: 4px; }
        .error-bar .bar { background: #dc3545; }
        .success-rate { color: #28a745; font-weight: bold; }
        .timestamp { color: #888; font-size: 14px; }
        .section { margin: 30px 0; }
        code { background: #f0f0f0; padding: 2px 6px; border-radius: 3px; font-size: 13px; }
        .badge { display: inline-block; padding: 3px 8px; border-radius: 4px; font-size: 12px; font-weight: 500; }
        .badge-success { background: #d4edda; color: #155724; }
        .badge-error { background: #f8d7da; color: #721c24; }
        .badge-info { background: #cce5ff; color: #004085; }
    </style>
</head>
<body>
    <h1>Fusillade Load Test Report</h1>
    <p class="timestamp">Generated: "#);

    // Add timestamp
    html.push_str(&chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
    html.push_str("</p>\n");

    // Summary stats
    let error_count: usize = report.errors.values().sum();
    let success_rate = if report.total_requests > 0 {
        ((report.total_requests - error_count) as f64 / report.total_requests as f64) * 100.0
    } else {
        0.0
    };

    html.push_str(
        r#"
    <h2>Summary</h2>
    <div class="summary-grid">"#,
    );

    // Total requests card
    html.push_str(&format!(
        r#"
        <div class="stat-card">
            <div class="label">Total Requests</div>
            <div class="value">{}</div>
        </div>"#,
        format_number(report.total_requests)
    ));

    // Success rate card
    let success_class = if success_rate >= 99.0 {
        "success"
    } else if success_rate >= 95.0 {
        "warning"
    } else {
        "error"
    };
    html.push_str(&format!(
        r#"
        <div class="stat-card {}">
            <div class="label">Success Rate</div>
            <div class="value">{:.1}%</div>
        </div>"#,
        success_class, success_rate
    ));

    // Avg latency card
    html.push_str(&format!(
        r#"
        <div class="stat-card">
            <div class="label">Avg Latency</div>
            <div class="value">{:.1}<span class="unit">ms</span></div>
        </div>"#,
        report.avg_latency_ms
    ));

    // P95 latency card
    html.push_str(&format!(
        r#"
        <div class="stat-card">
            <div class="label">P95 Latency</div>
            <div class="value">{:.1}<span class="unit">ms</span></div>
        </div>"#,
        report.p95_latency_ms
    ));

    // P99 latency card
    html.push_str(&format!(
        r#"
        <div class="stat-card">
            <div class="label">P99 Latency</div>
            <div class="value">{:.1}<span class="unit">ms</span></div>
        </div>"#,
        report.p99_latency_ms
    ));

    // Throughput card (requests per second)
    let rps = if report.total_duration_ms > 0 {
        (report.total_requests as f64 / report.total_duration_ms as f64) * 1000.0
    } else {
        0.0
    };
    html.push_str(&format!(
        r#"
        <div class="stat-card">
            <div class="label">Throughput</div>
            <div class="value">{:.1}<span class="unit">req/s</span></div>
        </div>"#,
        rps
    ));

    html.push_str("</div>\n");

    // Latency distribution
    html.push_str(
        r#"
    <div class="section">
        <h2>Latency Distribution</h2>
        <table>
            <tr>
                <th>Metric</th>
                <th>Value</th>
            </tr>"#,
    );

    html.push_str(&format!(
        r#"
            <tr><td>Minimum</td><td>{} ms</td></tr>
            <tr><td>Average</td><td>{:.2} ms</td></tr>
            <tr><td>P50 (Median)</td><td>{:.2} ms</td></tr>
            <tr><td>P90</td><td>{:.2} ms</td></tr>
            <tr><td>P95</td><td>{:.2} ms</td></tr>
            <tr><td>P99</td><td>{:.2} ms</td></tr>
            <tr><td>Maximum</td><td>{} ms</td></tr>"#,
        report.min_latency_ms,
        report.avg_latency_ms,
        report.p50_latency_ms,
        report.p90_latency_ms,
        report.p95_latency_ms,
        report.p99_latency_ms,
        report.max_latency_ms
    ));

    html.push_str("</table></div>\n");

    // Status codes
    if !report.status_codes.is_empty() {
        html.push_str(
            r#"
    <div class="section">
        <h2>Status Codes</h2>
        <table>
            <tr>
                <th>Code</th>
                <th>Count</th>
                <th>Percentage</th>
                <th>Distribution</th>
            </tr>"#,
        );

        let mut codes: Vec<_> = report.status_codes.iter().collect();
        codes.sort_by_key(|(code, _)| *code);

        for (code, count) in codes {
            let pct = (*count as f64 / report.total_requests as f64) * 100.0;
            let badge_class = if *code >= 200 && *code < 300 {
                "badge-success"
            } else if *code >= 400 {
                "badge-error"
            } else {
                "badge-info"
            };

            html.push_str(&format!(
                r#"
            <tr>
                <td><span class="badge {}">{}</span></td>
                <td>{}</td>
                <td>{:.1}%</td>
                <td><div class="bar-container"><div class="bar" style="width: {}%"></div></div></td>
            </tr>"#,
                badge_class,
                code,
                format_number(*count),
                pct,
                pct.min(100.0)
            ));
        }

        html.push_str("</table></div>\n");
    }

    // Per-endpoint breakdown (filter out internal iteration metrics)
    let real_endpoints: Vec<_> = report
        .grouped_requests
        .iter()
        .filter(|(name, _)| {
            !name.ends_with("iteration")
                && !name.ends_with("iteration_total")
                && *name != "iteration"
                && *name != "iteration_total"
        })
        .collect();

    if !real_endpoints.is_empty() {
        html.push_str(
            r#"
    <div class="section">
        <h2>Endpoints</h2>
        <table>
            <tr>
                <th>Endpoint</th>
                <th>Requests</th>
                <th>Avg (ms)</th>
                <th>P95 (ms)</th>
                <th>Min (ms)</th>
                <th>Max (ms)</th>
                <th>Errors</th>
            </tr>"#,
        );

        let mut endpoints = real_endpoints;
        endpoints.sort_by(|a, b| b.1.total_requests.cmp(&a.1.total_requests));

        for (name, req) in endpoints {
            let error_class = if req.error_count > 0 { "error-bar" } else { "" };
            let error_pct = if req.total_requests > 0 {
                (req.error_count as f64 / req.total_requests as f64) * 100.0
            } else {
                0.0
            };

            html.push_str(&format!(
                r#"
            <tr>
                <td><code>{}</code></td>
                <td>{}</td>
                <td>{:.1}</td>
                <td>{:.1}</td>
                <td>{}</td>
                <td>{}</td>
                <td class="{}">{} ({:.1}%)</td>
            </tr>"#,
                truncate_endpoint(name, 60),
                format_number(req.total_requests),
                req.avg_latency_ms,
                req.p95_latency_ms,
                req.min_latency_ms,
                req.max_latency_ms,
                error_class,
                req.error_count,
                error_pct
            ));
        }

        html.push_str("</table></div>\n");
    }

    // Errors
    if !report.errors.is_empty() {
        html.push_str(
            r#"
    <div class="section">
        <h2>Errors</h2>
        <table>
            <tr>
                <th>Error</th>
                <th>Count</th>
            </tr>"#,
        );

        let mut errors: Vec<_> = report.errors.iter().collect();
        errors.sort_by(|a, b| b.1.cmp(a.1));

        for (error, count) in errors {
            html.push_str(&format!(
                r#"
            <tr>
                <td><code>{}</code></td>
                <td>{}</td>
            </tr>"#,
                escape_html(error),
                format_number(*count)
            ));
        }

        html.push_str("</table></div>\n");
    }

    // Checks
    if !report.checks.is_empty() {
        html.push_str(
            r#"
    <div class="section">
        <h2>Checks</h2>
        <table>
            <tr>
                <th>Check</th>
                <th>Passed</th>
                <th>Failed</th>
                <th>Pass Rate</th>
            </tr>"#,
        );

        let mut checks: Vec<_> = report.checks.iter().collect();
        checks.sort_by_key(|(name, _)| name.as_str());

        for (name, (passed, failed)) in checks {
            let total = passed + failed;
            let pass_rate = if total > 0 {
                (*passed as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            let rate_class = if pass_rate >= 99.0 {
                "success-rate"
            } else {
                ""
            };

            html.push_str(&format!(
                r#"
            <tr>
                <td><code>{}</code></td>
                <td>{}</td>
                <td>{}</td>
                <td class="{}">{:.1}%</td>
            </tr>"#,
                escape_html(name),
                format_number(*passed),
                format_number(*failed),
                rate_class,
                pass_rate
            ));
        }

        html.push_str("</table></div>\n");
    }

    // Custom metrics
    if !report.counters.is_empty() || !report.gauges.is_empty() || !report.histograms.is_empty() {
        html.push_str(
            r#"
    <div class="section">
        <h2>Custom Metrics</h2>"#,
        );

        if !report.counters.is_empty() {
            html.push_str(
                r#"
        <h3>Counters</h3>
        <table>
            <tr><th>Name</th><th>Value</th></tr>"#,
            );
            for (name, value) in &report.counters {
                html.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{:.2}</td></tr>",
                    escape_html(name),
                    value
                ));
            }
            html.push_str("</table>");
        }

        if !report.gauges.is_empty() {
            html.push_str(
                r#"
        <h3>Gauges</h3>
        <table>
            <tr><th>Name</th><th>Value</th></tr>"#,
            );
            for (name, value) in &report.gauges {
                html.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{:.2}</td></tr>",
                    escape_html(name),
                    value
                ));
            }
            html.push_str("</table>");
        }

        if !report.histograms.is_empty() {
            html.push_str(
                r#"
        <h3>Histograms</h3>
        <table>
            <tr><th>Name</th><th>Avg</th><th>P95</th><th>P99</th><th>Min</th><th>Max</th></tr>"#,
            );
            for (name, hist) in &report.histograms {
                html.push_str(&format!(
                    "<tr><td><code>{}</code></td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td></tr>",
                    escape_html(name), hist.avg, hist.p95, hist.p99, hist.min, hist.max
                ));
            }
            html.push_str("</table>");
        }

        html.push_str("</div>\n");
    }

    // Data transfer
    html.push_str(&format!(
        r#"
    <div class="section">
        <h2>Data Transfer</h2>
        <table>
            <tr><td>Data Sent</td><td>{}</td></tr>
            <tr><td>Data Received</td><td>{}</td></tr>
        </table>
    </div>"#,
        format_bytes(report.total_data_sent),
        format_bytes(report.total_data_received)
    ));

    // Connection pool metrics
    let pool_total = report.pool_hits + report.pool_misses;
    if pool_total > 0 {
        let pool_rate = report.pool_hits as f64 / pool_total as f64 * 100.0;
        html.push_str(&format!(
            r#"
    <div class="section">
        <h2>Connection Pool</h2>
        <table>
            <tr><td>Reused Connections</td><td>{} ({:.1}%)</td></tr>
            <tr><td>New Connections</td><td>{}</td></tr>
        </table>
    </div>"#,
            report.pool_hits, pool_rate, report.pool_misses
        ));
    }

    // Footer
    html.push_str(r#"
    <footer style="margin-top: 40px; padding-top: 20px; border-top: 1px solid #ddd; color: #888; font-size: 14px;">
        Generated by <a href="https://fusillade.io" style="color: #e94560;">Fusillade</a> - High-performance load testing
    </footer>
</body>
</html>"#);

    html
}

fn format_number(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.2} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn truncate_endpoint(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_generate_html_empty() {
        let report = ReportStats {
            total_requests: 0,
            total_duration_ms: 0,
            avg_latency_ms: 0.0,
            min_latency_ms: 0,
            max_latency_ms: 0,
            p50_latency_ms: 0.0,
            p90_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            p99_latency_ms: 0.0,
            status_codes: HashMap::new(),
            errors: HashMap::new(),
            checks: HashMap::new(),
            grouped_requests: HashMap::new(),
            total_data_sent: 0,
            total_data_received: 0,
            histograms: HashMap::new(),
            rates: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            pool_hits: 0,
            pool_misses: 0,
        };
        let html = generate_html(&report);
        assert!(html.contains("No requests were made"));
    }

    #[test]
    fn test_generate_html_with_data() {
        let mut status_codes = HashMap::new();
        status_codes.insert(200, 950);
        status_codes.insert(500, 50);

        let report = ReportStats {
            total_requests: 1000,
            total_duration_ms: 10000,
            avg_latency_ms: 45.5,
            min_latency_ms: 10,
            max_latency_ms: 500,
            p50_latency_ms: 40.0,
            p90_latency_ms: 80.0,
            p95_latency_ms: 120.0,
            p99_latency_ms: 200.0,
            status_codes,
            errors: HashMap::new(),
            checks: HashMap::new(),
            grouped_requests: HashMap::new(),
            total_data_sent: 1024000,
            total_data_received: 5120000,
            histograms: HashMap::new(),
            rates: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            pool_hits: 0,
            pool_misses: 0,
        };

        let html = generate_html(&report);
        assert!(html.contains("1.0K")); // 1000 requests formatted
        assert!(html.contains("45.5")); // avg latency
        assert!(html.contains("200")); // status code
        assert!(html.contains("500")); // status code
        assert!(html.contains("Fusillade Load Test Report"));
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(500), "500");
        assert_eq!(format_number(1500), "1.5K");
        assert_eq!(format_number(1500000), "1.5M");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1572864), "1.50 MB");
        assert_eq!(format_bytes(1610612736), "1.50 GB");
    }

    #[test]
    fn test_escape_html() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
    }

    #[test]
    fn test_iteration_metrics_filtered() {
        use crate::stats::RequestReport;

        let mut grouped_requests = HashMap::new();

        // Add real endpoint
        grouped_requests.insert(
            "https://api.example.com/users".to_string(),
            RequestReport {
                total_requests: 100,
                min_latency_ms: 10,
                max_latency_ms: 200,
                avg_latency_ms: 50.0,
                p95_latency_ms: 150.0,
                p99_latency_ms: 180.0,
                error_count: 0,
                avg_blocked_ms: 0.0,
                avg_connecting_ms: 0.0,
                avg_tls_handshaking_ms: 0.0,
                avg_sending_ms: 0.0,
                avg_waiting_ms: 0.0,
                avg_receiving_ms: 0.0,
                avg_response_size: 0.0,
            },
        );

        // Add iteration metrics (should be filtered out)
        grouped_requests.insert(
            "iteration".to_string(),
            RequestReport {
                total_requests: 100,
                min_latency_ms: 10,
                max_latency_ms: 200,
                avg_latency_ms: 50.0,
                p95_latency_ms: 150.0,
                p99_latency_ms: 180.0,
                error_count: 100,
                avg_blocked_ms: 0.0,
                avg_connecting_ms: 0.0,
                avg_tls_handshaking_ms: 0.0,
                avg_sending_ms: 0.0,
                avg_waiting_ms: 0.0,
                avg_receiving_ms: 0.0,
                avg_response_size: 0.0,
            },
        );

        grouped_requests.insert(
            "iteration_total".to_string(),
            RequestReport {
                total_requests: 100,
                min_latency_ms: 10,
                max_latency_ms: 200,
                avg_latency_ms: 50.0,
                p95_latency_ms: 150.0,
                p99_latency_ms: 180.0,
                error_count: 100,
                avg_blocked_ms: 0.0,
                avg_connecting_ms: 0.0,
                avg_tls_handshaking_ms: 0.0,
                avg_sending_ms: 0.0,
                avg_waiting_ms: 0.0,
                avg_receiving_ms: 0.0,
                avg_response_size: 0.0,
            },
        );

        let report = ReportStats {
            total_requests: 100,
            total_duration_ms: 10000,
            avg_latency_ms: 50.0,
            min_latency_ms: 10,
            max_latency_ms: 200,
            p50_latency_ms: 45.0,
            p90_latency_ms: 80.0,
            p95_latency_ms: 150.0,
            p99_latency_ms: 180.0,
            status_codes: HashMap::new(),
            errors: HashMap::new(),
            checks: HashMap::new(),
            grouped_requests,
            total_data_sent: 1024,
            total_data_received: 5120,
            histograms: HashMap::new(),
            rates: HashMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            pool_hits: 0,
            pool_misses: 0,
        };

        let html = generate_html(&report);

        // Real endpoint should be present
        assert!(html.contains("api.example.com/users"));

        // Iteration metrics should be filtered out from endpoints table
        // They should NOT appear in the endpoints section
        let endpoints_section = html
            .find("<h2>Endpoints</h2>")
            .map(|start| &html[start..])
            .unwrap_or("");

        // The endpoints section shouldn't contain "iteration" as a standalone endpoint
        // But we need to be careful - "iteration" might appear in other contexts
        let section_end = endpoints_section
            .find("</table>")
            .unwrap_or(endpoints_section.len());
        let endpoints_table = &endpoints_section[..section_end];

        assert!(!endpoints_table.contains("<code>iteration</code>"));
        assert!(!endpoints_table.contains("<code>iteration_total</code>"));
    }
}
