// HTTP headers test scenario
// Tests custom headers, response header access, and header edge cases

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Test custom request headers
    let res1 = http.get('https://httpbin.org/headers', {
        headers: {
            'X-Custom-Header': 'test-value',
            'X-Request-ID': utils.uuid(),
            'Accept': 'application/json',
        },
        timeout: '5s',
    });
    let headers1 = JSON.parse(res1.body).headers;
    assertion(headers1, {
        'custom header sent': (h) => h['X-Custom-Header'] === 'test-value',
        'accept header sent': (h) => h['Accept'] === 'application/json',
    });

    // Test response headers access
    let res2 = http.get('https://httpbin.org/response-headers?X-Test=hello&X-Foo=bar', {
        timeout: '5s',
    });
    assertion(res2, {
        'response has custom headers': (r) => r.status === 200,
    });

    // Test User-Agent header
    let res3 = http.get('https://httpbin.org/user-agent', {
        headers: { 'User-Agent': 'Fusillade/Test' },
        timeout: '5s',
    });
    let ua = JSON.parse(res3.body);
    assertion(ua, {
        'user-agent is set': (u) => u['user-agent'] === 'Fusillade/Test',
    });

    sleep(0.5);
}
