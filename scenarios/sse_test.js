// SSE (Server-Sent Events) Test
// Tests sse.connect() and event streaming

export const options = {
    workers: 1,
    duration: '10s',
};

export default function () {
    // Connect to an SSE endpoint (replace with real endpoint for testing)
    // Example: https://sse.dev/test or a local mock server
    const client = sse.connect('https://sse.dev/test');

    if (!client) {
        print('Failed to connect to SSE endpoint');
        return;
    }

    print('Connected to SSE endpoint: ' + client.url);

    // Receive a few events
    for (let i = 0; i < 3; i++) {
        const event = client.recv();

        if (event === null) {
            print('SSE stream ended');
            break;
        }

        print('Received event: type=' + event.event + ', data=' + event.data);
    }

    // Clean up
    client.close();
    print('SSE connection closed');
}
