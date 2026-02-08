// HTTP compression test scenario
// Tests gzip/brotli/deflate response handling

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Test gzip compressed response
    let res1 = http.get('https://httpbin.org/gzip', {
        timeout: '5s',
    });
    assertion(res1, {
        'gzip returns 200': (r) => r.status === 200,
        'gzip body is decompressed': (r) => JSON.parse(r.body).gzipped === true,
    });

    // Test deflate compressed response
    let res2 = http.get('https://httpbin.org/deflate', {
        timeout: '5s',
    });
    assertion(res2, {
        'deflate returns 200': (r) => r.status === 200,
        'deflate body is decompressed': (r) => JSON.parse(r.body).deflated === true,
    });

    // Test brotli compressed response
    let res3 = http.get('https://httpbin.org/brotli', {
        timeout: '5s',
    });
    assertion(res3, {
        'brotli returns 200': (r) => r.status === 200,
        'brotli body is decompressed': (r) => JSON.parse(r.body).brotli === true,
    });

    sleep(0.5);
}
