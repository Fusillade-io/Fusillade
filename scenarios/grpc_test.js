// gRPC unary smoke test against a grpcbin-compatible hello service.
// Set FUSILLADE_GRPC_URL to point at a server (CI runs moul/grpcbin);
// defaults to a local grpcbin on the standard plaintext port.

const URL = __ENV.FUSILLADE_GRPC_URL || 'http://localhost:9000';

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'grpc_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

const client = new GrpcClient();
client.load(['scenarios/hello.proto'], ['scenarios/']);

export default function () {
    let ok = false;

    print('Connecting to gRPC at ' + URL + '...');
    try {
        client.connect(URL);

        let response = client.invoke('hello.HelloService/SayHello', {
            greeting: 'fusillade',
        });
        print('Reply: ' + response.reply);

        check(response, {
            'reply greets fusillade': (r) => r.reply === 'hello fusillade',
        });
        ok = response.reply === 'hello fusillade';
    } catch (e) {
        print('gRPC Error (server likely unavailable): ' + e);
    }

    metrics.rateAdd('grpc_success', ok);
}
