// Assertion Function Test Scenario
// Comprehensive testing of the assertion() function

export const options = {
    workers: 1,
    duration: '3s'
};

export default function () {
    print('Testing assertion() function...');

    // Test 1: HTTP response checking
    let res = http.get('https://httpbin.org/status/200');
    assertion(res, {
        'status is 200': (r) => r.status === 200,
        'response has body': (r) => r.body !== undefined
    });
    print('HTTP assertion completed');

    // Test 2: Multiple assertions on simple values
    let testValue = 42;
    assertion(testValue, {
        'is a number': (v) => typeof v === 'number',
        'is greater than 0': (v) => v > 0,
        'equals 42': (v) => v === 42
    });
    print('Value assertions completed');

    // Test 3: String checks
    let testString = 'Thruster Load Testing';
    assertion(testString, {
        'contains Thruster': (s) => s.includes('Thruster'),
        'length is correct': (s) => s.length === 21,
        'starts with T': (s) => s.startsWith('T')
    });
    print('String assertions completed');

    // Test 4: Object checks
    let testObject = { name: 'Thruster', version: '0.1.0' };
    assertion(testObject, {
        'has name property': (o) => o.name !== undefined,
        'name is Thruster': (o) => o.name === 'Thruster',
        'has version': (o) => o.version !== undefined
    });
    print('Object assertions completed');

    // Test 5: Array checks
    let testArray = [1, 2, 3, 4, 5];
    assertion(testArray, {
        'has 5 elements': (a) => a.length === 5,
        'first element is 1': (a) => a[0] === 1,
        'includes 3': (a) => a.includes(3)
    });
    print('Array assertions completed');

    print('All assertions tested successfully');
    sleep(0.5);
}
