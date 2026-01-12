export const options = {
  workers: 2,
  duration: '1s',
};

const users = new SharedArray('users', function () {
  const f = open('scenarios/users.json');
  return JSON.parse(f);
});

export default function () {
  const user = users.get(0);
  assertion(user.username, {
      'is user1': (v) => v === 'user1',
  });
  
  const len = users.length;
  assertion(len, {
      'len is 2': (v) => v === 2,
  });
}
