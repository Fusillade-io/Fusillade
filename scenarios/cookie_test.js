// Cookie handling test scenario
// Tests automatic cookie storage, sending, and jar management

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Set a cookie via response headers
    let res1 = http.get('https://httpbin.org/cookies/set/testcookie/testvalue', {
        timeout: '5s',
    });

    // Verify cookie is sent back automatically
    let res2 = http.get('https://httpbin.org/cookies', {
        timeout: '5s',
    });

    let cookies = JSON.parse(res2.body);
    assertion(cookies, {
        'cookie jar has testcookie': (c) => c.cookies && c.cookies.testcookie === 'testvalue',
    });

    // Set multiple cookies
    http.get('https://httpbin.org/cookies/set/session/abc123', { timeout: '5s' });
    let res3 = http.get('https://httpbin.org/cookies', { timeout: '5s' });
    let allCookies = JSON.parse(res3.body);
    assertion(allCookies, {
        'has session cookie': (c) => c.cookies && c.cookies.session === 'abc123',
    });

    sleep(0.5);
}
