// HTTP authentication test scenario
// Tests Basic auth and Bearer token patterns

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Test HTTP Basic Auth
    let res1 = http.get('https://httpbin.org/basic-auth/user/passwd', {
        headers: {
            'Authorization': 'Basic ' + encoding.b64encode('user:passwd'),
        },
        timeout: '5s',
    });
    assertion(res1, {
        'basic auth succeeds': (r) => r.status === 200,
        'authenticated as user': (r) => JSON.parse(r.body).authenticated === true,
    });

    // Test Bearer token pattern
    let res2 = http.get('https://httpbin.org/bearer', {
        headers: {
            'Authorization': 'Bearer my-test-token-123',
        },
        timeout: '5s',
    });
    assertion(res2, {
        'bearer auth succeeds': (r) => r.status === 200,
        'token is received': (r) => JSON.parse(r.body).authenticated === true,
    });

    // Test auth failure (no credentials)
    let res3 = http.get('https://httpbin.org/basic-auth/user/passwd', {
        timeout: '5s',
    });
    assertion(res3, {
        'no auth returns 401': (r) => r.status === 401,
    });

    sleep(0.5);
}
