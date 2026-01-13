// Test: Protocol error handling
// Verifies the system handles various HTTP error conditions gracefully

export const options = {
    workers: 2,
    iterations: 5,
};

export default function() {
    // Test 4xx errors
    let res404 = http.get('https://httpbin.org/status/404');
    check(res404, {
        '404 returns correct status': (r) => r.status === 404,
    });

    let res400 = http.get('https://httpbin.org/status/400');
    check(res400, {
        '400 returns correct status': (r) => r.status === 400,
    });

    // Test 5xx errors
    let res500 = http.get('https://httpbin.org/status/500');
    check(res500, {
        '500 returns correct status': (r) => r.status === 500,
    });

    let res503 = http.get('https://httpbin.org/status/503');
    check(res503, {
        '503 returns correct status': (r) => r.status === 503,
    });

    // Test redirect handling
    let resRedirect = http.get('https://httpbin.org/redirect/1');
    check(resRedirect, {
        'redirect followed successfully': (r) => r.status === 200,
    });

    // Test delayed response (timeout handling)
    let resDelay = http.get('https://httpbin.org/delay/1');
    check(resDelay, {
        'delayed response received': (r) => r.status === 200,
    });

    // Track error metrics
    metrics.counterAdd('error_test_requests', 1);

    sleep(0.2);
}
