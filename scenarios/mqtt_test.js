// MQTT publish smoke test.
// Defaults to the public test.mosquitto.org broker; set FUSILLADE_MQTT_HOST /
// FUSILLADE_MQTT_PORT to use a local broker (CI runs eclipse-mosquitto).

const HOST = __ENV.FUSILLADE_MQTT_HOST || 'test.mosquitto.org';
const PORT = parseInt(__ENV.FUSILLADE_MQTT_PORT || '1883');

export const options = {
    workers: 1,
    iterations: 1,
    thresholds: {
        'mqtt_success': ['rate >= 1'],
    },
    abort_on_fail: true,
};

export default function() {
    let client = new JsMqttClient();
    let ok = false;

    print('Connecting to MQTT at ' + HOST + ':' + PORT + '...');
    try {
        client.connect(HOST, PORT, 'fusillade-test-client-' + utils.uuid());
        print('MQTT Connected');

        client.publish('fusillade/test/topic', 'Hello from Fusillade!');
        print('MQTT Message Published');

        client.close();
        print('MQTT Connection Closed');
        ok = true;
    } catch (e) {
        print('MQTT Error: ' + e);
    }

    metrics.rateAdd('mqtt_success', ok);
}
