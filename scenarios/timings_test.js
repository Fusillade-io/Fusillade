// Test: Response Timings Object
// Verifies that HTTP response objects include timings for assertions
// Verifies that HTTP response objects include timings for assertions

export const options = {
    workers: 1,
    duration: '5s',
};

export default function () {
    const res = http.get('https://httpbin.org/get');

    // Check that timings object exists
    print('=== Testing Response Timings ===');
    print('Status: ' + res.status);
    print('Timings object: ' + JSON.stringify(res.timings));

    // Verify individual timing properties are numbers
    const timings = res.timings;

    check(res, {
        'status is 200': (r) => r.status === 200,
        'has timings object': (r) => r.timings !== undefined,
        'duration is a number': (r) => typeof r.timings.duration === 'number',
        'duration > 0': (r) => r.timings.duration > 0,
        'blocked is a number': (r) => typeof r.timings.blocked === 'number',
        'waiting is a number': (r) => typeof r.timings.waiting === 'number',
        'receiving is a number': (r) => typeof r.timings.receiving === 'number',
    });

    // Test timing assertions
    check(res, {
        'response time < 5000ms': (r) => r.timings.duration < 5000,
    });

    print('Duration: ' + timings.duration.toFixed(2) + 'ms');
    print('Waiting: ' + timings.waiting.toFixed(2) + 'ms');
    print('Receiving: ' + timings.receiving.toFixed(2) + 'ms');
    print('=== Timings test completed ===');

    sleep(1);
}
