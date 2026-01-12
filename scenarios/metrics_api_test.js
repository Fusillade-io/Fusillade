export const options = {
    workers: 1,
    duration: '2s'
};

export default function() {
    http.get('https://httpbin.org/get');
    
    // Test Metrics API
    let s = stats.get('https://httpbin.org/get');
    print('Current P95 for GET: ' + s.p95 + 'ms');
    print('Request count: ' + s.count);
    
    assertion(s.count > 0, { 'metrics collected': (v) => v === true });
    
    sleep(0.1);
}
