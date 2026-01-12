// Advanced HTTP test scenario
// Each worker will execute this script repeatedly
export const options = { duration: '1s' };

export default function () {
    // Test GET
    print('Worker fetching...');
    let res1 = http.get('https://httpbin.org/get', { timeout: '5s' });
    print('Worker status: ' + res1.status);

    assertion(res1, {
        'GET status is 200': (r) => r.status === 200,
        'GET body not empty': (r) => r.body.length > 0
    });

    // Test POST
    let res2 = http.post('https://httpbin.org/post', '{"foo":"bar"}', { headers: { "Content-Type": "application/json" } });
    assertion(res2, {
        'POST status is 200': (r) => r.status === 200,
    });

    // Test PUT
    let res3 = http.put('https://httpbin.org/put', 'new data');
    assertion(res3, {
        'PUT status is 200': (r) => r.status === 200,
    });

    // Test DELETE
    let res4 = http.del('https://httpbin.org/delete');
    assertion(res4, {
        'DELETE status is 200': (r) => r.status === 200,
    });

    sleep(1);
}