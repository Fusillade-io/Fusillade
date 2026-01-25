use crate::stats::ReportStats;

#[allow(dead_code)]
pub fn generate_html(report: &ReportStats) -> String {
    if report.total_requests == 0 {
        return "<html><body><h1>Fusillade Load Test Report</h1><p>No requests made</p></body></html>".to_string();
    }

    let mut html = String::new();
    html.push_str("<html><head><title>Fusillade Report</title></head><body>");
    html.push_str("<h1>Fusillade Load Test Report</h1>");
    html.push_str(&format!("<p>Total Requests: {}</p>", report.total_requests));
    html.push_str(&format!(
        "<p>Average Latency: {:.2} ms</p>",
        report.avg_latency_ms
    ));
    html.push_str(&format!(
        "<p>P95 Latency: {:.2} ms</p>",
        report.p95_latency_ms
    ));
    html.push_str(&format!(
        "<p>P99 Latency: {:.2} ms</p>",
        report.p99_latency_ms
    ));
    html.push_str("</body></html>");
    html
}
