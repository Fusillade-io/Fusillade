export default function() {
  let res = http.get("https://httpbin.org/get");
  assertion(res, {
    "status is 200": (r) => r.status === 200,
    "body is not empty": (r) => r.body.length > 0,
    "failing check": (r) => r.status === 500
  });
  sleep(0.1);
}
