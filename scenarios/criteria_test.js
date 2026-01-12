export const options = {
    workers: 1,
    duration: '2s',
    criteria: {
        'http_req_duration': ['p95 < 500', 'avg < 300'],
        'http_req_failed': ['rate < 0.1']
    }
};

export default function() {
    http.get('https://httpbin.org/get');
    sleep(0.1);
}