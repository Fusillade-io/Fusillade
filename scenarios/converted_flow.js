export const options = {
    workers: 1,
    duration: '10s'
};

export default function() {
    http.request({
        method: 'GET',
        url: 'https://httpbin.org/get?har=true',
        headers: {
            'Accept': 'application/json',
        },
    });
    sleep(0.1);
}
