// Integration test for HTTP protocol version (proto) field
export const options = {
    workers: 1,
    iterations: 1,
};

export default function() {
    // Test HTTP/2 endpoint (most modern services support h2)
    let res = http.get('https://httpbin.org/get');

    check(res, {
        'status is 200': (r) => r.status === 200,
        'proto field exists': (r) => r.proto !== undefined,
        'proto is h1 or h2': (r) => r.proto === 'h1' || r.proto === 'h2',
    });

    print('Response proto: ' + res.proto);

    // Test with POST request
    let postRes = http.post('https://httpbin.org/post', JSON.stringify({ test: 'data' }), {
        headers: { 'Content-Type': 'application/json' }
    });

    check(postRes, {
        'POST status is 200': (r) => r.status === 200,
        'POST proto field exists': (r) => r.proto !== undefined,
        'POST proto is h1 or h2': (r) => r.proto === 'h1' || r.proto === 'h2',
    });

    print('POST proto: ' + postRes.proto);
}
