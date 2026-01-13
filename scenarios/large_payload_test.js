// Test: Large payload handling
// Verifies the system handles large request/response bodies correctly

export const options = {
    workers: 2,
    iterations: 5,
};

export default function() {
    // Test large request body
    const largeBody = 'x'.repeat(100000); // 100KB payload

    let res = http.post('https://httpbin.org/post', largeBody, {
        headers: { 'Content-Type': 'text/plain' }
    });

    check(res, {
        'large POST status 200': (r) => r.status === 200,
        'response has data': (r) => r.body.length > 0,
    });

    // Test response with bytes endpoint (returns binary data)
    let bytesRes = http.get('https://httpbin.org/bytes/10000');
    check(bytesRes, {
        'bytes endpoint status 200': (r) => r.status === 200,
        'received expected bytes': (r) => r.body.length >= 10000,
    });

    // Test stream endpoint with moderate size
    let streamRes = http.get('https://httpbin.org/stream-bytes/50000');
    check(streamRes, {
        'stream status 200': (r) => r.status === 200,
    });

    sleep(0.5);
}
