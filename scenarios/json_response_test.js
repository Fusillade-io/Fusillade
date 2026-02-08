// JSON response parsing test scenario
// Tests JSON body parsing, nested objects, and .json() helper

export const options = {
    workers: 1,
    duration: '3s',
};

export default function() {
    // Test JSON response parsing
    let res1 = http.post('https://httpbin.org/post', JSON.stringify({
        name: 'Fusillade',
        version: '1.4.1',
        features: ['http', 'grpc', 'ws', 'mqtt'],
    }), {
        headers: { 'Content-Type': 'application/json' },
        timeout: '5s',
    });

    let body = JSON.parse(res1.body);
    assertion(body, {
        'has json field': (b) => b.json !== undefined,
        'name preserved': (b) => b.json.name === 'Fusillade',
        'version preserved': (b) => b.json.version === '1.4.1',
        'features is array': (b) => Array.isArray(b.json.features),
        'has 4 features': (b) => b.json.features.length === 4,
    });

    // Test nested JSON objects
    let res2 = http.post('https://httpbin.org/post', JSON.stringify({
        user: {
            name: 'Test User',
            address: {
                city: 'Helsinki',
                country: 'FI',
            },
        },
    }), {
        headers: { 'Content-Type': 'application/json' },
        timeout: '5s',
    });

    let body2 = JSON.parse(res2.body);
    assertion(body2, {
        'nested object preserved': (b) => b.json.user.address.city === 'Helsinki',
    });

    sleep(0.5);
}
