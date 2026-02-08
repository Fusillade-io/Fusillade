// HTTP status code handling test
// Tests various HTTP status codes

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // 2xx Success codes
    let res200 = http.get('https://httpbin.org/status/200', { timeout: '5s' });
    assertion(res200, { '200 OK': (r) => r.status === 200 });

    let res201 = http.post('https://httpbin.org/status/201', '', { timeout: '5s' });
    assertion(res201, { '201 Created': (r) => r.status === 201 });

    let res204 = http.del('https://httpbin.org/status/204', { timeout: '5s' });
    assertion(res204, { '204 No Content': (r) => r.status === 204 });

    // 4xx Client errors
    let res400 = http.get('https://httpbin.org/status/400', { timeout: '5s' });
    assertion(res400, { '400 Bad Request': (r) => r.status === 400 });

    let res403 = http.get('https://httpbin.org/status/403', { timeout: '5s' });
    assertion(res403, { '403 Forbidden': (r) => r.status === 403 });

    let res404 = http.get('https://httpbin.org/status/404', { timeout: '5s' });
    assertion(res404, { '404 Not Found': (r) => r.status === 404 });

    let res429 = http.get('https://httpbin.org/status/429', { timeout: '5s' });
    assertion(res429, { '429 Too Many Requests': (r) => r.status === 429 });

    // 5xx Server errors
    let res500 = http.get('https://httpbin.org/status/500', { timeout: '5s' });
    assertion(res500, { '500 Internal Server Error': (r) => r.status === 500 });

    let res502 = http.get('https://httpbin.org/status/502', { timeout: '5s' });
    assertion(res502, { '502 Bad Gateway': (r) => r.status === 502 });

    let res503 = http.get('https://httpbin.org/status/503', { timeout: '5s' });
    assertion(res503, { '503 Service Unavailable': (r) => r.status === 503 });

    sleep(0.5);
}
