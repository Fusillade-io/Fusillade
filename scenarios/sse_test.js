// SSE (Server-Sent Events) streaming test.
// Set FUSILLADE_SSE_URL to a local endpoint (CI uses jmalloc/echo-server's
// /.sse stream); defaults to the public sse.dev test endpoint.

const URL = __ENV.FUSILLADE_SSE_URL || 'https://sse.dev/test';

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'sse_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

export default function () {
    let received = 0;

    print('Connecting to SSE endpoint ' + URL + '...');
    try {
        const client = sse.connect(URL);
        if (!client) {
            print('Failed to connect to SSE endpoint');
            metrics.rateAdd('sse_success', false);
            return;
        }

        // Receive a few events; every event must carry data
        for (let i = 0; i < 3; i++) {
            const event = client.recv();
            if (event === null) {
                print('SSE stream ended early');
                break;
            }
            print('Received event: type=' + event.event + ', data=' + event.data);
            if (event.data !== undefined && event.data !== null) {
                received++;
            }
        }

        client.close();
        print('SSE connection closed');
    } catch (e) {
        print('SSE Error (server may be unavailable): ' + e);
    }

    metrics.rateAdd('sse_success', received >= 3);
}
