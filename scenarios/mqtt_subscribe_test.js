// MQTT Subscribe/Receive Test Scenario
// Demonstrates subscribing to topics and receiving messages
// Requires an MQTT broker (e.g., docker run -p 1883:1883 eclipse-mosquitto)

export const options = {
    workers: 1,
    duration: '10s'
};

export default function () {
    // Create two clients: one publisher, one subscriber
    const publisher = new JsMqttClient();
    const subscriber = new JsMqttClient();

    const clientId = utils.uuid().substring(0, 8);
    const testTopic = 'fusillade/test/' + clientId;

    try {
        // Connect subscriber first
        print('Connecting subscriber...');
        subscriber.connect('test.mosquitto.org', 1883, 'fusi-sub-' + clientId);
        subscriber.subscribe(testTopic);
        print('Subscribed to: ' + testTopic);

        // Connect publisher
        print('Connecting publisher...');
        publisher.connect('test.mosquitto.org', 1883, 'fusi-pub-' + clientId);

        // Give subscription time to register
        sleep(1);

        // Publish some test messages
        for (let i = 0; i < 3; i++) {
            const payload = JSON.stringify({ index: i, time: Date.now() });
            publisher.publish(testTopic, payload);
            print('Published: ' + payload);
            sleep(0.2);
        }

        // Receive messages (with timeout)
        print('\nReceiving messages...');
        let received = 0;
        let msg;

        // Try to receive messages (recv has 30s timeout, but we'll limit iterations)
        const startTime = Date.now();
        while (received < 3 && (Date.now() - startTime) < 5000) {
            msg = subscriber.recv();
            if (msg !== null) {
                received++;
                print(`Received [${msg.topic}] QoS=${msg.qos}: ${msg.payload}`);
            }
        }

        print(`\nReceived ${received} messages`);

        // Test wildcard subscription
        print('\n--- Wildcard Subscription Test ---');
        const wildcardTopic = 'fusillade/sensors/+/temperature';
        subscriber.subscribe(wildcardTopic);
        print('Subscribed to wildcard: ' + wildcardTopic);

        // Publish to matching topics
        publisher.publish('fusillade/sensors/room1/temperature', '22.5');
        publisher.publish('fusillade/sensors/room2/temperature', '23.1');
        sleep(0.5);

        // Receive wildcard matches
        for (let i = 0; i < 2; i++) {
            msg = subscriber.recv();
            if (msg !== null) {
                print(`Wildcard match: ${msg.topic} = ${msg.payload}`);
            }
        }

    } catch (e) {
        print('MQTT Error: ' + e);
    } finally {
        publisher.close();
        subscriber.close();
        print('\nConnections closed');
    }
}
