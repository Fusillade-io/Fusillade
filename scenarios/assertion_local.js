export default function() {
  let val = { status: 200, body: "hello" };
  assertion(val, {
    "status is 200": (r) => r.status === 200,
    "body is hello": (r) => r.body === "hello",
    "failing check": (r) => r.status === 500
  });
  sleep(0.1);
}
