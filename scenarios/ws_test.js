// WebSocket Test Scenario
// Tests WebSocket connection using a public echo server

export const options = {
    workers: 1,
    duration: '3s'
};

export default function () {
    print('WebSocket test starting...');

    try {
        // Connect to a public WebSocket echo server
        // Connect to a public WebSocket echo server
        // Using wss://echo.websocket.org
        let socket = ws.connect('wss://echo.websocket.org');
        print('WebSocket connected');

        // Send a message
        let testMessage = 'Hello from Fusillade!';
        socket.send(testMessage);
        print('Sent: ' + testMessage);

        // Receive the echo
        let received = socket.recv();
        print('Received: ' + received);

        // Verify the echo
        assertion(received === testMessage, {
            'echo matches sent message': (v) => v === true
        });

        // Close the connection
        socket.close();
        print('WebSocket closed');

    } catch (e) {
        // Graceful handling if WebSocket server is unavailable
        print('WebSocket Error (server may be unavailable): ' + e);
    }

    sleep(0.5);
}
