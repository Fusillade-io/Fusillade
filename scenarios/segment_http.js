export const options = {
  workers: 1,
  duration: '1s',
};

export default function () {
  segment('API', function () {
      http.get('http://localhost:1234/test');
  });
}
