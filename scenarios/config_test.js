// Test: Config file loading
// This test verifies that configuration can be loaded from external files
// Run with: fusillade run scenarios/config_test.js --config config/test_config.yaml

// Options can be overridden by config file
export const options = {
    workers: 1,
    duration: '5s',
};

export default function() {
    let res = http.get('https://httpbin.org/get');

    print(`Config test - Status: ${res.status}`);

    check(res, {
        'request successful': (r) => r.status === 200,
    });

    // The actual configuration values would be applied by the runtime
    // This test ensures the script runs correctly with config files

    sleep(0.5);
}
