export default function () {
    let user = __ENV.USER || 'unknown';
    let path = __ENV.PATH || 'unknown';
    let custom = __ENV.MY_VAR || 'default';

    print('User: ' + user);
    print('Path starts with: ' + path.substring(0, 20));
    print('Custom Var: ' + custom);

    assertion(custom, {
        'custom var matches': (v) => v === 'fusillade-test-value',
    });
}
