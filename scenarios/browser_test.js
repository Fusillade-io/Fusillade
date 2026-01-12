export const options = {
    duration: '5s',
    workers: 1,
};

export default function () {
    print('Launching browser...');
    const browser = chromium.launch();

    // Test 1: Navigation and Content (External)
    print('Test 1: Navigating to example.com...');
    const page = browser.newPage();
    page.goto('https://example.com');

    const content = page.content();
    assertion(content, {
        'contains Example Domain': (c) => c.includes('Example Domain')
    });

    // Test 2: Interaction (Data URL)
    print('Test 2: Interaction (Type, Click, Evaluate)...');
    // Simple interactive page
    const html = `
        <html><body>
            <input id="input" type="text" />
            <button id="btn" onclick="document.getElementById('result').innerText = document.getElementById('input').value + ' Clicked'">Submit</button>
            <div id="result"></div>
        </body></html>
    `;
    page.goto('data:text/html,' + encodeURIComponent(html));

    // Type text
    page.type('#input', 'Hello');

    // Click button
    page.click('#btn');

    // Evaluate result (check if click handler worked)
    const resultText = page.evaluate('document.getElementById("result").innerText');
    print('Interaction Result: ' + resultText);

    assertion(resultText, {
        'interaction worked': (t) => t === 'Hello Clicked'
    });

    // Test 3: Metrics
    print('Test 3: Performance Metrics...');
    const metrics = page.metrics();
    print('Navigation Start: ' + metrics.navigationStart);

    assertion(metrics, {
        'has navigationStart': (m) => m.navigationStart > 0
    });

    // Test 4: Screenshot
    print('Test 4: Taking screenshot...');
    const png = page.screenshot();
    print('Screenshot size: ' + png.length + ' bytes');
    assertion(png, {
        'screenshot taken': (p) => p.length > 0
    });

    browser.close();
    print('Browser closed.');
}
