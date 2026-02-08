// TypeScript basic test scenario
// Validates that TS type annotations are stripped correctly

interface Options {
    workers: number;
    duration: string;
}

export const options: Options = {
    workers: 1,
    duration: '1s',
};

function formatResult(status: number, body: string): string {
    return `Status: ${status}, Body length: ${body.length}`;
}

export default function(): void {
    const res = http.get('https://httpbin.org/get', { timeout: '5s' });

    assertion(res, {
        'status is 200': (r: any) => r.status === 200,
        'body not empty': (r: any) => r.body.length > 0,
    });

    const result: string = formatResult(res.status, res.body);
    print(result);

    sleep(0.1);
}
