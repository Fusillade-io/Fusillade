// Test: Export flags (--json, --export_json, --export_html)
// This test generates metrics that can be exported to different formats
// Run with:
//   fusillade run scenarios/export_test.js --json
//   fusillade run scenarios/export_test.js --export_json=/tmp/report.json
//   fusillade run scenarios/export_test.js --export_html=/tmp/report.html

export const options = {
    workers: 2,
    iterations: 10,
};

export default function() {
    // Generate various metrics for export testing

    // HTTP requests with different endpoints
    let res1 = http.get('https://httpbin.org/get');
    check(res1, {
        'GET status 200': (r) => r.status === 200,
    });

    let res2 = http.post('https://httpbin.org/post', JSON.stringify({ test: 'data' }), {
        headers: { 'Content-Type': 'application/json' }
    });
    check(res2, {
        'POST status 200': (r) => r.status === 200,
    });

    // Custom metrics that should appear in exports
    metrics.counterAdd('export_test_requests', 1);
    metrics.histogramAdd('export_test_custom_timing', Math.random() * 100);
    metrics.gaugeSet('export_test_active', utils.randomInt(1, 10));
    metrics.rateAdd('export_test_success', res1.status === 200);

    // Use segments for grouping in reports
    segment('api_calls', () => {
        let res = http.get('https://httpbin.org/headers');
        check(res, {
            'headers endpoint ok': (r) => r.status === 200,
        });
    });

    sleep(0.1);
}
