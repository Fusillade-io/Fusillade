// Tags Verification Test
// http, sleep are globals

export const options = {
    workers: 1,
    duration: '3s',
};

export default function () {
    print("Starting tags test...");

    // Request 1: Simple tag
    http.get('https://httpbin.org/get', {
        tags: { type: 'simple', service: 'httpbin' }
    });

    // Request 2: Different tags
    http.post('https://httpbin.org/post', 'data', {
        tags: { type: 'post_data', environment: 'test' }
    });

    // Request 3: Tag with overlapping name
    http.get('https://httpbin.org/get', {
        name: 'my_custom_name',
        tags: { region: 'us-east' }
    });

    sleep(1);
}
