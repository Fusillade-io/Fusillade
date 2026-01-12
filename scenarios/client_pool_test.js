export const options = {
    workers: 5,
    duration: '5s'
};

export default function() {
    let res = http.get('https://httpbin.org/get');
    assertion(res.status, {
        'status is 200': (s) => s === 200,
    });
}
