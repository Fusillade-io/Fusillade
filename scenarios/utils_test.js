export default function() {
    let id = utils.uuid();
    print('UUID: ' + id);
    assertion(id.length, { 'uuid length is 36': (l) => l === 36 });

    let num = utils.randomInt(1, 10);
    print('Random Int: ' + num);
    assertion(num >= 1 && num <= 10, { 'randomInt in range': (v) => v === true });

    let str = utils.randomString(8);
    print('Random String: ' + str);
    assertion(str.length, { 'randomString length': (l) => l === 8 });

    let item = utils.randomItem(['a', 'b', 'c']);
    print('Random Item: ' + item);
    assertion(['a', 'b', 'c'].indexOf(item) !== -1, { 'randomItem in array': (v) => v === true });
}
