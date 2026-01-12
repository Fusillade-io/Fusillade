export const options = {
    workers: 1,
    duration: '2s'
};

export default function() {
    let client = new JsMqttClient();
    
    print('Connecting to MQTT...');
    // test.mosquitto.org is a public broker
    try {
        client.connect('test.mosquitto.org', 1883, 'thruster-test-client-' + utils.uuid());
        print('MQTT Connected');

        client.publish('thruster/test/topic', 'Hello from Thruster!');
        print('MQTT Message Published');

        client.close();
        print('MQTT Connection Closed');
    } catch (e) {
        print('MQTT Error: ' + e);
    }
}
