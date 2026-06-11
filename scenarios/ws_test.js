// WebSocket echo round-trip test.
// Defaults to the public echo server; set FUSILLADE_WS_URL to use a local
// echo server (CI runs jmalloc/echo-server).

const URL = __ENV.FUSILLADE_WS_URL || 'wss://echo.websocket.org';

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'ws_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

export default function () {
    print('WebSocket test starting against ' + URL + '...');
    let ok = false;

    try {
        let socket = ws.connect(URL);
        print('WebSocket connected');

        let testMessage = 'Hello from Fusillade!';
        socket.send(testMessage);
        print('Sent: ' + testMessage);

        // Some echo servers (echo.websocket.org, jmalloc/echo-server) send a
        // greeting before echoing, so scan the first few messages.
        for (let i = 0; i < 3; i++) {
            let received = socket.recv();
            print('Received: ' + received);
            if (received === testMessage) {
                ok = true;
                break;
            }
        }

        socket.close();
        print('WebSocket closed');
    } catch (e) {
        print('WebSocket Error (server may be unavailable): ' + e);
    }

    metrics.rateAdd('ws_success', ok);
}
