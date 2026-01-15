// Response Sink Test Scenario
// Tests the response_sink option which discards response bodies to save memory.
// Body is still downloaded from network (for connection keep-alive) but not stored.

export const options = {
    workers: 1,
    duration: '5s',
    response_sink: true  // Enable response body sinking
};

export default function () {
    print('Testing response_sink feature...');

    // Test 1: GET request with sink enabled
    let res = http.get('https://httpbin.org/get');

    // Status should still be accessible
    assertion(res, {
        'status is 200': (r) => r.status === 200,
        'has headers': (r) => r.headers !== undefined && Object.keys(r.headers).length > 0
    });

    // Body should be empty/null when response_sink is enabled
    let bodyIsEmpty = res.body === null || res.body === '' || res.body.length === 0;
    assertion(bodyIsEmpty, {
        'body is discarded (empty)': (v) => v === true
    });
    print(`GET: status=${res.status}, body length=${res.body ? res.body.length : 0}`);

    // Test 2: POST request with sink enabled
    let postRes = http.post('https://httpbin.org/post', JSON.stringify({ test: 'data' }), {
        headers: { 'Content-Type': 'application/json' }
    });

    assertion(postRes, {
        'POST status is 200': (r) => r.status === 200,
        'POST has headers': (r) => r.headers !== undefined
    });

    let postBodyEmpty = postRes.body === null || postRes.body === '' || postRes.body.length === 0;
    assertion(postBodyEmpty, {
        'POST body is discarded': (v) => v === true
    });
    print(`POST: status=${postRes.status}, body length=${postRes.body ? postRes.body.length : 0}`);

    // Test 3: HEAD request (already has no body, but test for consistency)
    let headRes = http.head('https://httpbin.org/get');
    assertion(headRes, {
        'HEAD status is 200': (r) => r.status === 200,
        'HEAD body is empty': (r) => r.body === null || r.body === '' || r.body.length === 0
    });
    print(`HEAD: status=${headRes.status}`);

    // Test 4: Verify timings are still captured
    assertion(res.timings, {
        'timings exist': (t) => t !== undefined,
        'duration is positive': (t) => t.duration > 0
    });
    print(`Timings: duration=${res.timings.duration}ms`);

    print('Response sink tests completed successfully');
    sleep(0.5);
}
