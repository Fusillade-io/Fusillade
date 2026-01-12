export const options = {
    workers: 1,
    duration: '1s'
};

export default function() {
    // 1. Test Encoding
    let original = "Thruster Load Test";
    let encoded = encoding.b64encode(original);
    let decoded = encoding.b64decode(encoded);
    
    assertion(decoded, {
        'base64 roundtrip': (v) => v === original,
    });

    // 2. Test Crypto
    let data = "hello world";
    let m5 = crypto.md5(data);
    let s1 = crypto.sha1(data);
    let s256 = crypto.sha256(data);

    print('MD5: ' + m5);
    print('SHA1: ' + s1);
    print('SHA256: ' + s256);

    assertion(m5.length, { 'md5 length': (l) => l === 32 });
    assertion(s1.length, { 'sha1 length': (l) => l === 40 });
    assertion(s256.length, { 'sha256 length': (l) => l === 64 });
    
    // Known MD5 for "hello world"
    assertion(m5, { 'md5 matches': (v) => v === '5eb63bbbe01eeed093cb22bb8f5acdc3' });
}
