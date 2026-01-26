// gRPC Streaming Test Scenario
// Demonstrates all three streaming patterns: server, client, and bidirectional
// Requires a gRPC server with streaming methods

export const options = {
    workers: 1,
    duration: '5s'
};

const client = new GrpcClient();

// Load protos - adjust paths to your proto files
try {
    client.load(['scenarios/streaming.proto'], ['scenarios/']);
    print("Streaming proto loaded");
} catch (e) {
    print("Proto load failed: " + e);
}

export default function () {
    try {
        client.connect('http://localhost:50051');
        print('Connected to gRPC server');
    } catch (e) {
        print('Connection failed (server likely unavailable): ' + e);
        return;
    }

    // --- Server Streaming Example ---
    // Server sends multiple messages in response to a single request
    try {
        print('\n--- Server Streaming Test ---');
        const serverStream = client.serverStream('streaming.DataService/StreamData', {
            query: 'test-query',
            count: 5
        });

        let msgCount = 0;
        let msg;
        while ((msg = serverStream.recv()) !== null) {
            msgCount++;
            print(`Server stream message ${msgCount}: ${JSON.stringify(msg)}`);
        }
        print(`Server stream completed with ${msgCount} messages`);
    } catch (e) {
        print('Server streaming error: ' + e);
    }

    // --- Client Streaming Example ---
    // Client sends multiple messages, server responds once after all received
    try {
        print('\n--- Client Streaming Test ---');
        const clientStream = client.clientStream('streaming.DataService/UploadData');

        // Send multiple chunks
        for (let i = 0; i < 5; i++) {
            clientStream.send({ chunk: `data-chunk-${i}`, index: i });
            print(`Sent chunk ${i}`);
        }

        // Close send side and get aggregated response
        const response = clientStream.closeAndRecv();
        print(`Upload complete. Server response: ${JSON.stringify(response)}`);
    } catch (e) {
        print('Client streaming error: ' + e);
    }

    // --- Bidirectional Streaming Example ---
    // Both client and server can send messages independently
    try {
        print('\n--- Bidirectional Streaming Test ---');
        const bidiStream = client.bidiStream('streaming.DataService/Chat');

        // Send a few messages and receive responses
        for (let i = 0; i < 3; i++) {
            bidiStream.send({ message: `Hello ${i}`, timestamp: Date.now() });
            print(`Sent message ${i}`);

            const reply = bidiStream.recv();
            if (reply !== null) {
                print(`Received reply: ${JSON.stringify(reply)}`);
            }
        }

        bidiStream.close();
        print('Bidi stream closed');
    } catch (e) {
        print('Bidirectional streaming error: ' + e);
    }

    sleep(0.5);
}
