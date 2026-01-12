export const options = {
    workers: 1,
    duration: '2s'
};

export default function() {
    // These two unique URLs should be grouped under 'get_user'
    http.get('https://httpbin.org/get?id=1', { name: 'get_user' });
    http.get('https://httpbin.org/get?id=2', { name: 'get_user' });
    
    // This one should be grouped under its own URL (default)
    http.get('https://httpbin.org/get?other=true');

    sleep(0.1);
}
