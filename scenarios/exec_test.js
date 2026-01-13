// Test: exec command (single execution mode)
// This test verifies that scripts run correctly in exec mode
// Run with: fusillade exec scenarios/exec_test.js

let execCount = 0;

export default function() {
    execCount++;

    // Make a simple HTTP request
    let res = http.get('https://httpbin.org/get');

    print(`Exec run #${execCount}`);
    print(`Status: ${res.status}`);

    // Verify basic functionality works in exec mode
    check(res, {
        'status is 200': (r) => r.status === 200,
        'has headers': (r) => r.headers !== undefined,
        'has body': (r) => r.body.length > 0,
    });

    // Test utils work in exec mode
    let uuid = utils.uuid();
    print(`Generated UUID: ${uuid}`);

    let randomNum = utils.randomInt(1, 100);
    print(`Random number: ${randomNum}`);

    // Test encoding works
    let encoded = encoding.b64encode('hello exec mode');
    let decoded = encoding.b64decode(encoded);
    print(`Encoding roundtrip: ${decoded}`);

    assertion(decoded, {
        'encoding works': (v) => v === 'hello exec mode'
    });
}
