export const options = {
  workers: 1,
  duration: '1s',
};

export default function () {
  segment('Login', function () {
      assertion(200, {
          'status is 200': (v) => v === 200,
      });
  });
  
  segment('Checkout', function () {
     segment('Payment', function () {
         assertion(200, {
             'paid': (v) => v === 200,
         });
     });
  });
}
