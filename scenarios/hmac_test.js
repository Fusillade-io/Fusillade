// HMAC Test Scenario  
// Tests HMAC cryptographic functions

export const options = {
    workers: 1,
    duration: '2s'
};

export default function () {
    print('Testing HMAC functions...');

    let data = 'The quick brown fox jumps over the lazy dog';
    let key = 'secret-key';

    // Test HMAC-SHA256
    // Note: Expected values can be verified with:
    // echo -n "The quick brown fox jumps over the lazy dog" | openssl dgst -sha256 -hmac "secret-key"
    let hmacSha256 = crypto.hmac('sha256', key, data);
    print('HMAC-SHA256: ' + hmacSha256);

    // Verify HMAC is 64 chars (256 bits = 32 bytes = 64 hex chars)
    assertion(hmacSha256.length, {
        'HMAC-SHA256 length is 64': (l) => l === 64
    });

    // Verify it's a valid hex string
    assertion(/^[0-9a-f]+$/.test(hmacSha256), {
        'HMAC-SHA256 is valid hex': (v) => v === true
    });

    // Test HMAC-SHA1
    let hmacSha1 = crypto.hmac('sha1', key, data);
    print('HMAC-SHA1: ' + hmacSha1);

    // Verify HMAC-SHA1 is 40 chars (160 bits = 20 bytes = 40 hex chars)
    assertion(hmacSha1.length, {
        'HMAC-SHA1 length is 40': (l) => l === 40
    });

    // Test HMAC-MD5
    let hmacMd5 = crypto.hmac('md5', key, data);
    print('HMAC-MD5: ' + hmacMd5);

    // Verify HMAC-MD5 is 32 chars (128 bits = 16 bytes = 32 hex chars)
    assertion(hmacMd5.length, {
        'HMAC-MD5 length is 32': (l) => l === 32
    });

    // Test consistency - same input should produce same output
    let hmacSha256Again = crypto.hmac('sha256', key, data);
    assertion(hmacSha256 === hmacSha256Again, {
        'HMAC is deterministic': (v) => v === true
    });

    // Different key should produce different HMAC
    let differentKey = crypto.hmac('sha256', 'different-key', data);
    assertion(hmacSha256 !== differentKey, {
        'Different key produces different HMAC': (v) => v === true
    });

    print('HMAC tests completed successfully');
    sleep(1);
}
