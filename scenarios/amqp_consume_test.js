// AMQP Consume/Ack Test Scenario
// Demonstrates queue consumption and message acknowledgment
// Requires RabbitMQ: docker run -d -p 5672:5672 rabbitmq:3

export const options = {
    workers: 1,
    duration: '10s'
};

export default function () {
    const publisher = new JsAmqpClient();
    const consumer = new JsAmqpClient();

    const queueName = 'fusillade-test-' + utils.uuid().substring(0, 8);

    try {
        // Connect both clients
        print('Connecting to AMQP broker...');
        publisher.connect('amqp://guest:guest@127.0.0.1:5672');
        consumer.connect('amqp://guest:guest@127.0.0.1:5672');
        print('Connected');

        // Subscribe consumer to queue (this also declares the queue)
        print('Setting up consumer on queue: ' + queueName);
        consumer.subscribe(queueName);

        // Publish some messages
        print('\nPublishing messages...');
        for (let i = 0; i < 5; i++) {
            const payload = JSON.stringify({
                event: 'test.event',
                index: i,
                timestamp: Date.now()
            });
            publisher.publish('', queueName, payload);  // Default exchange, routing key = queue name
            print('Published: ' + payload);
        }

        // Give messages time to be queued
        sleep(0.5);

        // Consume and acknowledge messages
        print('\nConsuming messages...');
        let consumed = 0;
        let msg;

        while (consumed < 5) {
            msg = consumer.recv();
            if (msg === null) {
                print('No more messages (timeout)');
                break;
            }

            consumed++;
            print(`Received [tag=${msg.deliveryTag}]: ${msg.body}`);

            // Parse and validate
            const data = JSON.parse(msg.body);
            if (data.event === 'test.event') {
                // Acknowledge successful processing
                consumer.ack(msg.deliveryTag);
                print(`  -> Acknowledged`);
            } else {
                // Reject and requeue unexpected messages
                consumer.nack(msg.deliveryTag, true);
                print(`  -> Rejected (requeued)`);
            }
        }

        print(`\nConsumed and acknowledged ${consumed} messages`);

        // Test message rejection
        print('\n--- Rejection Test ---');
        const rejectPayload = JSON.stringify({ event: 'will.reject', id: 999 });
        publisher.publish('', queueName, rejectPayload);
        sleep(0.2);

        msg = consumer.recv();
        if (msg !== null) {
            print(`Received for rejection: ${msg.body}`);
            consumer.nack(msg.deliveryTag, false);  // Reject without requeue
            print('Message rejected (discarded)');
        }

    } catch (e) {
        print('AMQP Error (broker likely unavailable): ' + e);
    } finally {
        publisher.close();
        consumer.close();
        print('\nConnections closed');
    }
}
