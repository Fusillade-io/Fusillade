export const options = {
  workers: 1,
  duration: '1s',
};

export default function () {
  // Simulating a failed check
  assertion(500, {
    'status is 200': (val) => val === 200,
  });
  sleep(0.1);
}