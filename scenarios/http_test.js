// Advanced HTTP test scenario
// Each worker will execute this script repeatedly
const BASE = __ENV.FUSILLADE_BASE_URL || 'https://httpbin.org';

export const options = { duration: '1s' };

export default function () {
    // Test GET
    print('Worker fetching...');
    let res1 = http.get(BASE + '/get', { timeout: '5s' });
    print('Worker status: ' + res1.status);

    assertion(res1, {
        'GET status is 200': (r) => r.status === 200,
        'GET body not empty': (r) => r.body.length > 0
    });

    // Test POST
    let res2 = http.post(BASE + '/post', '{"foo":"bar"}', { headers: { "Content-Type": "application/json" } });
    assertion(res2, {
        'POST status is 200': (r) => r.status === 200,
    });

    // Test PUT
    let res3 = http.put(BASE + '/put', 'new data');
    assertion(res3, {
        'PUT status is 200': (r) => r.status === 200,
    });

    // Test DELETE
    let res4 = http.del(BASE + '/delete');
    assertion(res4, {
        'DELETE status is 200': (r) => r.status === 200,
    });

    sleep(1);
}