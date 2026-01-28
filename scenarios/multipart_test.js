// Multipart Upload Test
// Tests http.file() and multipart form-data submission



export const options = {
    workers: 1,
    duration: '5s',
};

export default function () {
    // Create a file object
    const file = http.file('./scenarios/users.json', 'users.json', 'application/json');

    // Submit as multipart
    // Note: http.post expects a string body. Fusillade automatically detects multipart
    // if the JSON string contains the file marker return by http.file().
    const res = http.post('https://httpbin.org/post', JSON.stringify({
        'file': file,
        'field1': 'value1',
    }), { name: 'MultipartUpload' });

    check(res, {
        'status is 200': (r) => r.status === 200,
        'file received': (r) => r.body.includes('users.json'),
    });
}
