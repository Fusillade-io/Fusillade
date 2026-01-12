// Segment Test Scenario
// Tests the segment() function for organizing test code

export const options = {
    workers: 1,
    duration: '3s'
};

export default function () {
    print('Testing segment() function...');

    // Test 1: Basic segment
    segment('Login Flow', function () {
        print('Inside Login Flow segment');

        let loginData = { username: 'testuser', password: 'testpass' };
        assertion(loginData.username.length > 0, {
            'username is not empty': (v) => v === true
        });
    });

    // Test 2: Nested segments
    segment('Checkout Process', function () {
        print('Inside Checkout Process segment');

        segment('Add to Cart', function () {
            print('Inside Add to Cart subsegment');

            let cartItem = { id: 123, quantity: 2 };
            assertion(cartItem.quantity, {
                'quantity is valid': (q) => q > 0
            });
        });

        segment('Payment', function () {
            print('Inside Payment subsegment');

            let payment = { amount: 99.99, currency: 'USD' };
            assertion(payment.currency, {
                'currency is USD': (c) => c === 'USD'
            });
        });
    });

    // Test 3: Segment with HTTP requests
    segment('API Requests', function () {
        print('Inside API Requests segment');

        let res = http.get('https://httpbin.org/get');
        assertion(res.status, {
            'API returns 200': (s) => s === 200
        });
    });

    // Test 4: Sequential segments
    segment('Step 1', function () {
        print('Executing Step 1');
    });

    segment('Step 2', function () {
        print('Executing Step 2');
    });

    segment('Step 3', function () {
        print('Executing Step 3');
    });

    print('Segment tests completed');
    sleep(0.2);
}
