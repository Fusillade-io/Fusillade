// TypeScript lifecycle hooks test
// Validates that setup/teardown work with TS type annotations

interface SetupData {
    token: string;
    timestamp: number;
}

export function setup(): SetupData {
    print('EXEC_SETUP');
    return { token: 'test-token-123', timestamp: Date.now() };
}

export default function(data: SetupData): void {
    print('EXEC_DEFAULT: ' + data.token);
    assertion(data, {
        'has token': (d: SetupData) => d.token === 'test-token-123',
        'has timestamp': (d: SetupData) => d.timestamp > 0,
    });
    sleep(0.01);
}

export function teardown(data: SetupData): void {
    print('EXEC_TEARDOWN: ' + data.token);
}
