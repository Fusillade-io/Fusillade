export const options = {
    workers: 1,
    duration: '5s',
    minIterationDuration: '1s'
};

export default function() {
    print('Iteration started at ' + new Date().toISOString());
    http.get('https://httpbin.org/get');
    // Iteration will be padded to 1s
}
