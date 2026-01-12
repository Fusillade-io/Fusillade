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

export default async function () {
    // This part requires a real server. 
    // Uncomment and adapt when a server is available.

    try {
        await client.connect('http://localhost:50051');

        let response = await client.invoke('helloworld.Greeter/SayHello', {
            name: 'Thruster'
        });

        print('Greeting: ' + response.message);

        assertion(response.message, {
            'greeting is correct': (m) => m === 'Hello Thruster'
        });
    } catch (e) {
        print('gRPC Error (server likely unavailable): ' + e);
    }

    sleep(1);
}
