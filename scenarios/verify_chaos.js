// Chaos Injection Verification Test
// http and print are globals in Fusillade

export const options = {
    workers: 1,
    duration: '5s',
};

export default function () {
    const start = Date.now();
    let res = http.get('https://httpbin.org/delay/0');
    const duration = Date.now() - start;

    if (res.status === 200) {
        print(`Request successful. Duration: ${duration}ms`);
    } else if (res.status === 0) {
        print(`Request dropped (simulated). Duration: ${duration}ms Error: ${res.body}`);
    } else {
        print(`Request returned status ${res.status}`);
    }
}
