// Browser automation smoke test.
//
// Test 1 navigates to a real page over the network: in CI this is the local
// httpbin service container (via FUSILLADE_BASE_URL), otherwise example.com.
// Tests 2-4 are fully self-contained data: URLs and need no network.
//
// The run gates on a `browser_success` Rate with abort_on_fail, so any failure
// (or an environment with no usable Chrome) exits non-zero instead of printing
// and passing — matching the ws/mqtt/amqp/sse/grpc scenarios.

const BASE = __ENV.FUSILLADE_BASE_URL;
const NAV_URL = BASE ? BASE + '/html' : 'https://example.com';
const NAV_NEEDLE = BASE ? 'Herman Melville' : 'Example Domain';

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'browser_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

export default function () {
    let ok = false;

    try {
        print('Launching browser...');
        const browser = chromium.launch();

        // Test 1: Navigation, content, and performance metrics on a real page.
        // (navigation timing is only meaningful for an actual page load, not a
        // data: URL, so metrics must be read here before navigating away.)
        print('Test 1: Navigating to ' + NAV_URL + '...');
        const page = browser.newPage();
        page.goto(NAV_URL);
        const content = page.content();
        if (!content.includes(NAV_NEEDLE)) {
            throw new Error('navigation content missing "' + NAV_NEEDLE + '"');
        }
        const m = page.metrics();
        print('navigationStart: ' + (m && m.navigationStart));
        if (!(m && m.navigationStart > 0)) {
            throw new Error('metrics missing navigationStart');
        }

        // Test 2: Interaction (type, click, evaluate) against a data: URL.
        print('Test 2: Interaction...');
        const html = `
            <html><body>
                <input id="input" type="text" />
                <button id="btn" onclick="document.getElementById('result').innerText = document.getElementById('input').value + ' Clicked'">Submit</button>
                <div id="result"></div>
            </body></html>
        `;
        page.goto('data:text/html,' + encodeURIComponent(html));
        page.type('#input', 'Hello');
        page.click('#btn');
        const resultText = page.evaluate('document.getElementById("result").innerText');
        print('Interaction Result: ' + resultText);
        if (resultText !== 'Hello Clicked') {
            throw new Error('interaction failed, got: ' + resultText);
        }

        // Test 3: Screenshot.
        print('Test 3: Screenshot...');
        const png = page.screenshot();
        print('Screenshot size: ' + png.length + ' bytes');
        if (!(png.length > 0)) {
            throw new Error('empty screenshot');
        }

        browser.close();
        print('Browser closed.');
        ok = true;
    } catch (e) {
        print('Browser test error (Chrome may be unavailable): ' + e);
    }

    metrics.rateAdd('browser_success', ok);
}
