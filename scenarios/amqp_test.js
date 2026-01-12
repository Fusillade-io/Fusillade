export const options = {
    workers: 1,
    duration: '2s'
};

export default function() {
    let client = new JsAmqpClient();
    
    print('Connecting to AMQP...');
    try {
        // Assuming RabbitMQ is running locally for this test
        client.connect('amqp://127.0.0.1:5672');
        print('AMQP Connected');

        client.publish('', 'test_queue', 'Hello from Thruster AMQP!');
        print('AMQP Message Published');

        client.close();
        print('AMQP Connection Closed');
    } catch (e) {
        print('AMQP Error (likely no local broker): ' + e);
    }
}
