export const options = {
    executor: 'constant-arrival-rate',
    rate: 5,
    timeUnit: '1s',
    duration: '5s',
    workers: 10 // max concurrent workers
};

export default function() {
    print('Iteration started at ' + new Date().toISOString());
    http.get('https://httpbin.org/get');
    sleep(0.5);
}
