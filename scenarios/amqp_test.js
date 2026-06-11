// AMQP publish smoke test.
// Requires a broker; set FUSILLADE_AMQP_URL or run RabbitMQ locally
// (CI runs a rabbitmq service container).

const URL = __ENV.FUSILLADE_AMQP_URL || 'amqp://127.0.0.1:5672';

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'amqp_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

export default function() {
    let client = new JsAmqpClient();
    let ok = false;

    print('Connecting to AMQP at ' + URL + '...');
    try {
        client.connect(URL);
        print('AMQP Connected');

        client.publish('', 'test_queue', 'Hello from Fusillade AMQP!');
        print('AMQP Message Published');

        client.close();
        print('AMQP Connection Closed');
        ok = true;
    } catch (e) {
        print('AMQP Error (likely no broker): ' + e);
    }

    metrics.rateAdd('amqp_success', ok);
}
