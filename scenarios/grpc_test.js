// gRPC Test Scenario
// Requires a running gRPC server (e.g., using grpc-reflection or known proto)

// Example using the classic Greeter service
// You would need 'helloworld.proto' locally.

export const options = {
    workers: 1,
    duration: '1s'
};

const client = new GrpcClient();

// Setup: Load protos and connect
// In a real test, you'd probably do this in setup() or top-level if simple
try {
    client.load(['scenarios/helloworld.proto'], ['scenarios/']);
    print("Proto loaded successfully");
} catch (e) {
    print("Proto load failed (expected if file missing): " + e);
}

export default function () {
    // This part requires a real server.
    // Uncomment and adapt when a server is available.

    try {
        client.connect('http://localhost:50051');

        let response = client.invoke('helloworld.Greeter/SayHello', {
            name: 'Fusillade'
        });

        print('Greeting: ' + response.message);

        assertion(response.message, {
            'greeting is correct': (m) => m === 'Hello Fusillade'
        });
    } catch (e) {
        print('gRPC Error (server likely unavailable): ' + e);
    }

    sleep(1);
}
