// HTTP redirect test scenario
// Tests automatic redirect following and redirect limits

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Test automatic redirect following
    let res1 = http.get('https://httpbin.org/redirect/2', {
        timeout: '5s',
    });
    assertion(res1, {
        'followed redirects to 200': (r) => r.status === 200,
    });

    // Test absolute redirect
    let res2 = http.get('https://httpbin.org/absolute-redirect/1', {
        timeout: '5s',
    });
    assertion(res2, {
        'absolute redirect returns 200': (r) => r.status === 200,
    });

    // Test redirect to specific URL
    let res3 = http.get('https://httpbin.org/redirect-to?url=https%3A%2F%2Fhttpbin.org%2Fget', {
        timeout: '5s',
    });
    assertion(res3, {
        'redirect-to returns 200': (r) => r.status === 200,
        'body has url field': (r) => r.body.includes('"url"'),
    });

    sleep(0.5);
}
