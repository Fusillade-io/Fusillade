export const options = {
    workers: 1,
    duration: '2s'
};

export default function() {
    let client = new JsMqttClient();
    
    print('Connecting to MQTT...');
    // test.mosquitto.org is a public broker
    try {
        client.connect('test.mosquitto.org', 1883, 'fusillade-test-client-' + utils.uuid());
        print('MQTT Connected');

        client.publish('fusillade/test/topic', 'Hello from Fusillade!');
        print('MQTT Message Published');

        client.close();
        print('MQTT Connection Closed');
    } catch (e) {
        print('MQTT Error: ' + e);
    }
}
