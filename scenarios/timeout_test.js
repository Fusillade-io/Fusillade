// Timeout Verification Test
// http and check, sleep are globals

export default function () {
    print("Starting timeout test...");

    // Test 1: Should timeout (3s delay, 1s timeout)
    print("Test 1: Expecting timeout...");
    try {
        let r1 = http.get('https://httpbin.org/delay/3', { timeout: '1s' });
        // Expecting status 0 and specific error message body
        if (r1.status === 0 && r1.body && r1.body.indexOf("request timeout") !== -1) {
            print("PASS: Request timed out as expected.");
        } else {
            print("FAIL: Expected timeout/status 0, but got status " + r1.status + " body: " + r1.body);
        }
    } catch (e) {
        // In case bridge throws instead of returning error object
        print("PASS: Caught failure (timeout): " + e);
    }

    // Test 2: Should succeed (1s delay, 5s timeout)
    print("Test 2: Expecting success...");
    try {
        let r2 = http.get('https://httpbin.org/delay/1', { timeout: '5s' });
        if (r2.status === 200) {
            print("PASS: Request succeeded within timeout.");
        } else {
            print("FAIL: Expected success (200), but got status " + r2.status + " body: " + r2.body);
        }
    } catch (e) {
        print("FAIL: Caught unexpected error in success test: " + e);
    }
}
