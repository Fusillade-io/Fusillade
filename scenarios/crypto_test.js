// Crypto module test scenario
// Tests md5, sha1, sha256, hmac, and encoding functions

export const options = {
    workers: 1,
    duration: '1s',
};

export default function() {
    // Test MD5
    let md5Hash = crypto.md5('hello');
    assertion(md5Hash, {
        'md5 produces 32-char hex': (h) => h.length === 32,
        'md5 is correct': (h) => h === '5d41402abc4b2a76b9719d911017c592',
    });

    // Test SHA1
    let sha1Hash = crypto.sha1('hello');
    assertion(sha1Hash, {
        'sha1 produces 40-char hex': (h) => h.length === 40,
        'sha1 is correct': (h) => h === 'aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d',
    });

    // Test SHA256
    let sha256Hash = crypto.sha256('hello');
    assertion(sha256Hash, {
        'sha256 produces 64-char hex': (h) => h.length === 64,
        'sha256 is correct': (h) => h === '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824',
    });

    // Test HMAC
    let hmacSha256 = crypto.hmac('sha256', 'secret', 'message');
    assertion(hmacSha256, {
        'hmac-sha256 produces hex string': (h) => h.length === 64,
    });

    // Test Base64 encoding/decoding
    let encoded = encoding.b64encode('Hello, World!');
    assertion(encoded, {
        'base64 encode': (e) => e === 'SGVsbG8sIFdvcmxkIQ==',
    });

    let decoded = encoding.b64decode(encoded);
    assertion(decoded, {
        'base64 roundtrip': (d) => d === 'Hello, World!',
    });

    sleep(0.1);
}
