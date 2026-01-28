# Fusillade: The Rust Load Testing Platform

**Fusillade** is a modern, high-performance load testing platform built for engineering teams who demand precision and efficiency. It combines the raw speed and memory safety of **Rust** with the developer-friendly scripting of **JavaScript**.

---

## 1. Core Philosophy & Architecture

* **Performance (The Host):** The engine handles networking, threading, and resource management using Rust's sync ecosystem (`std::thread`) and `reqwest::blocking` heavily tuned for HTTP/2 multiplexing.
* **Scripting (The Guest):** Users define test logic in standard JavaScript (ES Modules). The JS context is strictly for logic; it does not handle I/O directly.
* **Precision:** Metrics capture raw network + body download time, explicitly excluding JS execution overhead and internal processing.
* **Stability:** Explicit lifecycle management (`std::mem::forget`) prevents panics during runtime shutdown, ensuring clean exit codes.

---

## 2. Technology Stack

* **Language:** Rust (2021 edition)
* **JS Runtime:** `rquickjs` (QuickJS bindings)
* **HTTP:** `hyper` + `hyper-rustls` (HTTP/1.1 & HTTP/2, connection pooling, TLS)
* **WebSockets:** `tungstenite` (with rustls TLS)
* **gRPC:** `tonic`, `prost`, `prost-reflect` (Dynamic reflection)
* **IoT Protocols:** `rumqttc` (MQTT), `lapin` (AMQP)
* **SSE:** `reqwest::blocking` (Server-Sent Events streaming)
* **Browser:** `headless_chrome` (Chromium automation)
* **CLI:** `clap` (derive macros)
* **Config:** `serde` (YAML/JSON), JS `options` export
* **Metrics:** `hdrhistogram` (High-precision histograms)
* **Telemetry:** `opentelemetry-otlp` (OTLP export)
* **Database:** `rusqlite` (Test history storage)

---

## 3. Usage Guide

### Installation

```bash
cargo install --path .
```

### Quick Start

Initialize a new project with a sample script and configuration:

```bash
fusillade init -o my-test.js --config
```

### Running a Test

To run a load test, you need a flow script. Configuration is extracted from the `export const options` in the script.

```bash
fusillade run scenarios/test.js
```

> [!TIP]
> You can use the shorter alias `fusi` for all commands (e.g., `fusi run scenarios/test.js`).


### Writing a Test Scenario

Fusillade supports modern ES Modules and local imports.

**File: `scenarios/checkout.js`**

```javascript
// Fusillade provides http, check, sleep as globals


export const options = {
    // 1. Define Load Stages (Ramping)
    stages: [
        { duration: '30s', target: 20 }, // Ramp up to 20 users
        { duration: '1m', target: 20 },  // Stay at peak
        { duration: '10s', target: 0 },  // Ramp down
    ],
    // 2. Define Failure Criteria (Thresholds for CI/CD)
    thresholds: {
        'http_req_duration': ['p95 < 500'], // 95% of requests must complete < 500ms
        'http_req_failed': ['rate < 0.01'], // < 1% Error rate
    }
};

export default function () {
    const payload = JSON.stringify({ item_id: "12345", quantity: 1 });
    const headers = { 'Content-Type': 'application/json' };

    const res = http.post('https://api.example.com/cart', payload, { headers, name: 'AddToCart' });
    
    // Assertions
    check(res, {
        'status is 200': (r) => r.status === 200,
        'response time OK': (r) => r.timings.duration < 500,
    });

    sleep(1);
}
```

### Lifecycle Hooks

Fusillade supports optional lifecycle functions for test setup and teardown:

#### `setup()` - Run Once Before Test

The `setup` function runs once before any VUs start. Use it for:
- One-time authentication (get tokens, session cookies)
- Loading test data from files or APIs
- Initializing shared state

If `setup` returns a value, it's passed to every `default` function iteration:

```javascript
export function setup() {
    // Runs once before test starts
    const loginRes = http.post('https://api.example.com/login', {
        username: 'testuser',
        password: 'secret'
    });

    const token = JSON.parse(loginRes.body).token;

    // Return data to share with all VUs
    return {
        authToken: token,
        testData: JSON.parse(open('./users.json'))
    };
}

export default function(data) {
    // data contains what setup() returned
    http.get('https://api.example.com/profile', {
        headers: { 'Authorization': `Bearer ${data.authToken}` }
    });
}
```

#### `teardown(data)` - Run Once After Test

The `teardown` function runs once after all VUs finish. Use it for cleanup:

```javascript
export function teardown(data) {
    // Cleanup: logout, delete test data, etc.
    http.post('https://api.example.com/logout', null, {
        headers: { 'Authorization': `Bearer ${data.authToken}` }
    });
    print('Test completed, cleaned up resources');
}
```

**Lifecycle Flow:**
```
setup() → [VU iterations run in parallel] → teardown(data) → handleSummary(data)
   ↓              ↓                              ↓                    ↓
 1x only    N workers × M iterations         1x only              1x only
```

#### `handleSummary(data)` - Custom Summary Output

The `handleSummary` function runs after the test completes and receives the full test report data. It can generate custom output files (JSON, HTML, CSV, etc.):

```javascript
export function handleSummary(data) {
    // data contains:
    // - data.duration_secs: test duration
    // - data.total_requests: total request count
    // - data.total_errors: total error count
    // - data.avg_latency, data.p50, data.p95, data.p99: latency metrics
    // - data.rps: requests per second
    // - data.endpoints: per-endpoint breakdown

    return {
        // Write to stdout
        'stdout': `Test completed: ${data.total_requests} requests, ${data.rps} RPS`,

        // Write JSON summary
        'summary.json': JSON.stringify(data, null, 2),

        // Write custom HTML report
        'report.html': `<html><body><h1>Test Results</h1><p>RPS: ${data.rps}</p></body></html>`,
    };
}
```

**Return Value:**
- Keys are filenames (use `'stdout'` for console output)
- Values are the content to write
- Multiple files can be generated in a single return

This enables custom reporting formats, integration with external systems, and automated CI/CD result publishing.

### Executor Types

Fusillade supports multiple executor types for different load patterns:

| Executor | Description | Use Case |
|----------|-------------|----------|
| `constant-vus` | Maintain a fixed number of virtual users (default) | Baseline load testing |
| `ramping-vus` | Gradually adjust VU count through stages | Load ramp-up/down |
| `constant-arrival-rate` | Maintain a fixed request rate (RPS) | API rate limit testing |
| `ramping-arrival-rate` | Adjust request rate through stages | Variable RPS testing |
| `per-vu-iterations` | Each VU runs a fixed number of iterations | Iteration-based testing |
| `shared-iterations` | All VUs share a pool of iterations | Fixed total iterations |

```javascript
export const options = {
    // Constant VUs (default)
    executor: 'constant-vus',
    workers: 10,
    duration: '1m',
};

// Ramping VUs - gradually scale VU count
export const options = {
    executor: 'ramping-vus',
    stages: [
        { duration: '30s', target: 20 },  // Ramp to 20 VUs
        { duration: '1m', target: 50 },   // Ramp to 50 VUs
        { duration: '30s', target: 0 },   // Ramp down
    ],
};

// Constant Arrival Rate - fixed RPS
export const options = {
    executor: 'constant-arrival-rate',
    rate: 100,           // 100 requests per second
    timeUnit: '1s',      // Rate denominator
    workers: 20,         // Worker pool to sustain rate
    duration: '2m',
};

// Ramping Arrival Rate - variable RPS
export const options = {
    executor: 'ramping-arrival-rate',
    stages: [
        { duration: '30s', target: 50 },   // Ramp to 50 RPS
        { duration: '1m', target: 200 },   // Ramp to 200 RPS
        { duration: '30s', target: 0 },    // Ramp down
    ],
    workers: 50,         // Worker pool
};

// Per-VU Iterations - fixed iterations per VU
export const options = {
    executor: 'per-vu-iterations',
    workers: 10,
    iterations: 100,     // Each VU runs exactly 100 iterations
};
```

**Note:** For arrival rate executors, workers act as a pool that picks up iteration requests. The engine spawns iterations at the target rate, and available workers execute them. If all workers are busy, iterations are dropped (tracked as `dropped_iterations` metric).

### Execution Modes

#### 1. Local Run (Ad-hoc)

Basic execution with formatted terminal output.

```bash
fusillade run scenarios/checkout.js
```

#### 2. CI/CD Mode (Headless)

Runs without the TUI, outputs JSON logs for ingestion, and generates JUnit XML reports.

```bash
fusillade run scenarios/checkout.js --headless --out junit=results.xml
```

#### 3. CSV Export

Export metrics to a CSV file for analysis.

```bash
fusillade run scenarios/checkout.js --out csv=metrics.csv
```

#### 4. Distributed Mode (Cluster)

Coordinate multiple worker nodes to generate massive load.

**Worker:**

```bash
fusillade worker --listen 0.0.0.0:8080
```

**Controller:**

```bash
fusillade controller --listen 0.0.0.0:9000
```

Workers connect to the controller, and tests are dispatched via the controller's API. See the Kubernetes Deployment section for the full distributed architecture.

### HAR Conversion

Convert browser recordings (.har) directly into Fusillade flows:

```bash
fusillade convert --input recording.har --output flow.js
```

---

## 4. Configuration Options

Fusillade is configured via the `export const options` object in your script.

| Option | Type | Description | Example |
| :--- | :--- | :--- | :--- |
| `workers` | Number | Number of concurrent virtual users (VUs). | `20` |
| `duration` | String | Total duration of the test. | `'30s'`, `'1m'` |
| `stages` | Array | Ramping schedule (target VUs over time). | `[{ duration: '10s', target: 50 }]` |
| `thresholds` | Object | Pass/fail criteria. | `{ 'http_req_duration': ['p95 < 500'] }` |
| `stop` | String | Wait time for active iterations to finish after test duration expires. Defaults to `'30s'`. | `'30s'` |
| `iterations` | Number | Fixed number of iterations per worker (per-vu-iterations executor). If set, workers run exactly this many iterations then exit. | `10` |
| `warmup` | String | URL to hit for connection pool warmup before test starts. | `'https://api.example.com'` |
| `min_iteration_duration` | String | Minimum time each iteration must take. If an iteration finishes faster, it waits before starting the next. Useful for rate limiting. | `'1s'` |
| `scenarios` | Object | Multiple named scenarios with independent configs (see below). | `{ fast: {...}, slow: {...} }` |
| `stack_size` | Number | Worker thread stack size in bytes. Default: `32768` (32KB). Increase if encountering stack overflows with complex scripts. | `65536` |
| `response_sink` | Boolean | Sink (discard) response bodies to save memory. See [Response Sink Mode](#response-sink-mode) below. | `true` |
| `abort_on_fail` | Boolean | Abort the test immediately if any threshold is breached. Useful for CI/CD to fail fast. When enabled, the test exits with a non-zero status code on threshold failure. | `true` |
| `jitter` | String | Chaos: Add artificial latency before each request. | `'500ms'` |
| `drop` | Number | Chaos: Drop probability 0.0-1.0 for simulating packet loss. | `0.05` |
| `no_endpoint_tracking` | Boolean | Disable per-URL metrics tracking to reduce memory for high-cardinality URLs. | `true` |
| `memory_safe` | Boolean | Throttle worker spawning when memory usage is high (85% pause, 95% stop). | `true` |

### Config Aliases

Alternative option names are supported:

| Alias | Primary Name |
|-------|--------------|
| `vus` | `workers` |
| `stages` | `schedule` |
| `thresholds` | `criteria` |
| `timeUnit` | `time_unit` |
| `responseSink` | `response_sink` |
| `startTime` | `start_time` |

Both naming conventions work interchangeably.

### Scenarios

Run multiple user types or workflows concurrently with independent configurations:

```javascript
export const options = {
    scenarios: {
        browse: {
            workers: 10,
            duration: '30s',
            exec: 'browseProducts',
        },
        checkout: {
            workers: 5,
            duration: '1m',
            exec: 'checkoutFlow',
            startTime: '30s',  // starts after 30s delay
        },
    },
};

export function browseProducts() { /* browsing logic */ }
export function checkoutFlow() { /* checkout logic */ }
```

**Scenario Options:**
| Option | Type | Description |
|--------|------|-------------|
| `workers` | Number | Workers for this scenario |
| `duration` | String | Duration of this scenario |
| `iterations` | Number | Fixed iterations per worker |
| `exec` | String | Function name to call (default: `"default"`) |
| `startTime` | String | Delay before starting (e.g., `"30s"`) |
| `thresholds` | Object | Per-scenario pass/fail criteria |
| `stack_size` | Number | Worker stack size in bytes (default: 32KB) |
| `response_sink` | Boolean | Discard response bodies for this scenario |

### Response Sink Mode

When testing at high concurrency with large response bodies, memory usage can become a bottleneck. The `response_sink` option addresses this by discarding response bodies immediately after download.

**How it works:**
- **Network**: The full response body is still downloaded from the server (required for HTTP connection keep-alive/reuse)
- **Memory**: Body bytes are discarded immediately instead of being stored
- **Response object**: `response.body` will be `null` or empty when sink mode is enabled
- **Metrics**: Response size is still tracked correctly for bandwidth metrics

**When to use:**
- High-throughput tests (1000+ workers) with large response bodies
- Tests where you only care about status codes and headers, not body content
- Memory-constrained environments

**Example:**

```javascript
export const options = {
    workers: 1000,
    duration: '5m',
    response_sink: true,  // or responseSink (camelCase)
};

export default function() {
    const res = http.get('https://api.example.com/large-payload');

    // These still work:
    check(res, {
        'status is 200': (r) => r.status === 200,
        'has content-type': (r) => r.headers['content-type'] !== undefined,
    });

    // This will be empty/null when response_sink is enabled:
    // res.body
}
```

**Per-scenario configuration:**

```javascript
export const options = {
    scenarios: {
        heavy_load: {
            workers: 1000,
            duration: '5m',
            response_sink: true,  // Only sink bodies for this scenario
            exec: 'heavyTest',
        },
        validation: {
            workers: 10,
            duration: '5m',
            // response_sink defaults to false - bodies are available
            exec: 'validateResponses',
        },
    },
};
```

---

## 5. Configuration Files

Fusillade supports external configuration files in **YAML** or **JSON** format. These are useful for:
- Separating test logic (script) from test parameters (config)
- Reusing configurations across different scripts
- Environment-specific settings (local, staging, production)
- CI/CD pipelines where parameters vary

### Using Config Files

Config files work alongside (or override) the `export const options` in your script:

```bash
# Use a config file
fusillade run script.js --config config/stress-test.yaml

# CLI flags override config file values
fusillade run script.js --config config/stress-test.yaml --workers 100
```

### Available Config Examples

The `config/` directory includes ready-to-use templates:

| File | Purpose |
|------|---------|
| `local.yaml/json` | Quick smoke tests during development |
| `ramping.yaml/json` | Gradual load increase to find performance characteristics |
| `stress-test.yaml/json` | Push system beyond normal capacity |
| `soak-test.yaml/json` | Extended duration to detect memory leaks |
| `chaos.yaml/json` | Fault injection (latency, dropped requests) |
| `arrival-rate.yaml/json` | Fixed request rate (RPS) testing |
| `multi-scenario.yaml/json` | Multiple user behaviors concurrently |

### Test Types Explained

#### Load Test (Ramping)
Gradually increases virtual users to understand how performance degrades under load.

```yaml
# config/ramping.yaml
schedule:
  - duration: 30s
    target: 10      # Ramp to 10 VUs
  - duration: 1m
    target: 10      # Hold at 10 VUs
  - duration: 30s
    target: 50      # Ramp to 50 VUs
  - duration: 2m
    target: 50      # Hold at 50 VUs
  - duration: 30s
    target: 0       # Ramp down

criteria:
  http_req_duration:
    - "p95 < 500"
    - "avg < 200"
  http_req_failed:
    - "rate < 0.01"
```

#### Stress Test
Pushes the system beyond normal operating capacity to find breaking points.

```yaml
# config/stress-test.yaml
schedule:
  - duration: 30s
    target: 50
  - duration: 30s
    target: 100
  - duration: 30s
    target: 200     # Extreme load
  - duration: 2m
    target: 200
  - duration: 1m
    target: 0

# More lenient thresholds - expect some degradation
criteria:
  http_req_duration:
    - "p99 < 2000"
  http_req_failed:
    - "rate < 0.05"
```

#### Soak Test (Endurance)
Runs at moderate load for extended periods to detect:
- Memory leaks
- Resource exhaustion
- Connection pool issues
- Performance degradation over time

```yaml
# config/soak-test.yaml
workers: 25
duration: 1h              # Long duration
warmup: https://api.example.com/health

criteria:
  http_req_duration:
    - "p95 < 300"
    - "avg < 150"
  http_req_failed:
    - "rate < 0.001"      # Very strict - sustained load should be stable
```

#### Constant Arrival Rate
Maintains a fixed request rate regardless of response times. Useful for:
- API rate limit testing
- SLA verification
- Capacity planning

```yaml
# config/arrival-rate.yaml
executor: constant-arrival-rate
rate: 100                 # 100 requests per second
time_unit: 1s
workers: 50               # Worker pool to sustain the rate
duration: 2m

criteria:
  http_req_duration:
    - "p95 < 200"
```

#### Chaos Engineering
Tests system resilience by injecting faults:

```yaml
# config/chaos.yaml
workers: 20
duration: 5m

# Fault injection
jitter: 250ms             # Add 0-250ms random latency
drop: 0.03                # Drop 3% of requests

# Account for injected failures in thresholds
criteria:
  http_req_failed:
    - "rate < 0.10"       # 3% dropped + real failures
```

#### Multi-Scenario
Simulates realistic traffic with different user behaviors:

```yaml
# config/multi-scenario.yaml
scenarios:
  browsing:
    executor: constant-vus
    workers: 20
    duration: 5m
    exec: browseProducts

  checkout:
    executor: constant-vus
    workers: 5
    duration: 4m
    start_time: 1m        # Start after 1 minute delay
    exec: checkoutFlow
    thresholds:
      http_req_failed:
        - "rate < 0.005"
```

### JSON Equivalent

All YAML configs have JSON equivalents. Example:

```json
{
  "workers": 20,
  "duration": "5m",
  "jitter": "250ms",
  "drop": 0.03,
  "criteria": {
    "http_req_duration": ["p95 < 1000"],
    "http_req_failed": ["rate < 0.10"]
  }
}
```

### Threshold Syntax

Thresholds define pass/fail criteria for CI/CD integration:

| Syntax | Description |
|--------|-------------|
| `p95 < 500` | 95th percentile under 500ms |
| `p99 < 1000` | 99th percentile under 1000ms |
| `avg < 200` | Average under 200ms |
| `min > 0` | Minimum greater than 0 |
| `max < 5000` | Maximum under 5000ms |
| `rate < 0.01` | Rate under 1% (for error rates) |
| `count > 100` | Count greater than 100 |

**Note:** Spaces around operators (`<`, `>`) are required.

Multiple thresholds can be combined - all must pass:

```yaml
criteria:
  http_req_duration:
    - "p95 < 500"
    - "p99 < 1000"
    - "avg < 200"
  http_req_failed:
    - "rate < 0.01"
```

---

## 6. Global API Reference

### http Module

* `http.get(url, [options])`: Performs a GET request.
* `http.post(url, body, [options])`: Performs a POST request. Supports multipart automatically if `body` contains `__fusillade_file` markers (see `http.file`).
* `http.put(url, body, [options])`: Performs a PUT request.
* `http.patch(url, body, [options])`: Performs a PATCH request.
* `http.del(url, [options])`: Performs a DELETE request.
* `http.head(url, [options])`: Performs a HEAD request (returns headers only).
* `http.options(url, [options])`: Performs an OPTIONS request (for CORS preflight).
* `http.file(path, [filename], [contentType])`: Reads a file and returns a JSON string marker for multipart uploads.
* `http.url(baseUrl, [params])`: Build URL with query parameters. Returns URL string.
* `http.formEncode(obj)`: Encode object as `application/x-www-form-urlencoded` string.
* `http.basicAuth(username, password)`: Generate Basic auth header value (returns `"Basic base64..."`).
* `http.bearerToken(token)`: Generate Bearer auth header value (returns `"Bearer token"`).
* `http.setDefaults(options)`: Set global defaults for all requests (timeout, headers).
* `http.request({ method, url, body, headers, name, timeout })`: Generic request builder.
* `http.batch(requests)`: Execute multiple requests in parallel. Returns array of responses.
* `http.cookieJar()`: Returns the cookie jar for manual cookie management.
* `http.addHook(hookType, fn)`: Register a request/response hook.
* `http.clearHooks()`: Clear all registered hooks.

```javascript
// http.url() example
const url = http.url('https://api.example.com/search', { q: 'test', page: 1 });
// Returns: https://api.example.com/search?q=test&page=1

// http.formEncode() example
const body = http.formEncode({ username: 'user', password: 'pass' });
http.post('https://api.example.com/login', body, {
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' }
});

// http.batch() example - parallel requests
const responses = http.batch([
    { method: 'GET', url: 'https://api.example.com/users' },
    { method: 'GET', url: 'https://api.example.com/posts' },
    { method: 'POST', url: 'https://api.example.com/log', body: JSON.stringify({ event: 'test' }) }
]);
// responses is an array of Response objects in the same order
console.log(responses[0].status, responses[1].status, responses[2].status);

// http.basicAuth() and http.bearerToken() examples
http.get('https://api.example.com/private', {
    headers: { 'Authorization': http.basicAuth('user', 'pass') }
});

const token = 'eyJhbGciOiJIUzI1NiIs...';
http.get('https://api.example.com/me', {
    headers: { 'Authorization': http.bearerToken(token) }
});

// http.setDefaults() example - set global defaults for all requests
http.setDefaults({
    timeout: '10s',
    headers: {
        'X-API-Key': __ENV.API_KEY,
        'Accept': 'application/json'
    }
});
// All subsequent requests will use these defaults
http.get('https://api.example.com/data');  // Uses 10s timeout and default headers

// http.addHook() example - intercept requests and responses
http.addHook('beforeRequest', (req) => {
    console.log(`Making ${req.method} request to ${req.url}`);
    // req contains: method, url, body, headers
});

http.addHook('afterResponse', (res) => {
    console.log(`Got status ${res.status} in ${res.timings?.duration}ms`);
    // res contains: status, body, headers, proto
});

// Clear hooks when done
http.clearHooks();

// FormData example - multipart form uploads
const fd = new FormData();
fd.append('field', 'value');
fd.append('file', http.file('./upload.txt', 'upload.txt', 'text/plain'));

http.post('https://api.example.com/upload', fd.body(), {
    headers: { 'Content-Type': fd.contentType() }
});
```

### Request Options

The optional `options` argument supports:
* `headers` (Object): Custom HTTP headers (e.g., `{ 'Content-Type': 'application/json' }`).
* `name` (String): Custom metric tag for aggregating dynamic URLs.
* `tags` (Object): Custom tags for filtering/aggregating metrics (e.g., `{ type: 'checkout', region: 'us-east' }`).
* `timeout` (String): Request timeout duration (e.g., `'10s'`, `'500ms'`). Defaults to `'60s'`. Returns status `0` and error `"request timeout"` on failure.
* `retry` (Number): Number of retry attempts on failure. Defaults to `0`. Uses exponential backoff.
* `retryDelay` (Number): Initial retry delay in milliseconds. Defaults to `100`. Doubles each retry (up to 32x).
* `retryOn` (Function): Custom function to determine if request should be retried. Receives response object, returns boolean.
* `retryDelayFn` (Function): Custom function to calculate retry delay. Receives retry count (1-based), returns delay in milliseconds.
* `followRedirects` (Boolean): Whether to follow HTTP redirects (3xx). Defaults to `true`.
* `maxRedirects` (Number): Maximum number of redirects to follow. Defaults to `5`.

```javascript
// Retry failed requests with exponential backoff
const res = http.request({
    method: 'GET',
    url: 'https://api.example.com/flaky-endpoint',
    retry: 3,        // Retry up to 3 times
    retryDelay: 200  // Start with 200ms, then 400ms, 800ms
});

// Custom retry conditions - retry on rate limit or server errors
const res = http.request({
    method: 'GET',
    url: 'https://api.example.com/data',
    retry: 5,
    retryOn: (r) => r.status === 429 || r.status >= 500,  // Retry on 429 or 5xx
    retryDelayFn: (count) => count * 1000  // Linear backoff: 1s, 2s, 3s...
});
```

### Response Object

The `Response` object returned by requests contains:
* `status` (Number): HTTP status code (e.g., 200, 404). `0` for network/timeout errors.
* `statusText` (String): HTTP status text (e.g., "OK", "Not Found", "Network Error").
* `body` (String): Response body as text. **Note:** Will be `null`/empty when `response_sink` is enabled.
* `json()` (Function): Parses body as JSON and returns the result. Equivalent to `JSON.parse(res.body)`.
* `bodyContains(str)` (Function): Returns `true` if body contains the given string.
* `bodyMatches(pattern)` (Function): Returns `true` if body matches the regex pattern.
* `hasHeader(name, [value])` (Function): Returns `true` if header exists. If `value` is provided, also checks the header value matches.
* `isJson()` (Function): Returns `true` if body is valid JSON.
* `headers` (Object): Response headers.
* `cookies` (Object): Response cookies (parsed from Set-Cookie headers).
* `error` (String): Error type for failed requests: `TIMEOUT`, `DNS`, `TLS`, `CONNECT`, `RESET`, `NETWORK`.
* `errorCode` (String): Error code for failed requests: `ETIMEDOUT`, `ENOTFOUND`, `ECERT`, `ECONNREFUSED`, `ECONNRESET`, `EPIPE`, `ENETWORK`.
* `proto` (String): HTTP protocol version:
    * `"h1"`: HTTP/0.9, HTTP/1.0, HTTP/1.1
    * `"h2"`: HTTP/2
    * `"h3"`: HTTP/3
* `timings` (Object): Detailed timing metrics (in ms):
    * `duration`: Total request time.
    * `blocked`: Time waiting for a clearer connection slot.
    * `connecting`: TCP connection time.
    * `tls_handshaking`: TLS handshake time.
    * `sending`: Time sending the request.
    * `waiting`: Time waiting for the first byte (TTFB).
    * `receiving`: Time reading the response.

```javascript
// Using res.json() for cleaner code
const res = http.get('https://api.example.com/users');
const data = res.json();  // Instead of JSON.parse(res.body)
print(data.users.length);

// Using response matchers for cleaner checks
check(res, {
    'status is 200': (r) => r.status === 200,
    'body contains success': (r) => r.bodyContains('success'),
    'body matches pattern': (r) => r.bodyMatches(/order-\d+/),
    'has content-type': (r) => r.hasHeader('content-type'),
    'content-type is json': (r) => r.hasHeader('content-type', 'application/json'),
    'response is valid json': (r) => r.isJson(),
});
```

**Automatic URL Naming:**

Fusillade automatically normalizes URLs for metrics by replacing numeric and UUID segments with `:id`:

```javascript
// These requests will all be grouped under the same metric name:
http.get('/users/123');           // Metric: GET /users/:id
http.get('/users/456');           // Metric: GET /users/:id
http.get('/orders/abc-def-123');  // Metric: GET /orders/:id (UUID detected)

// Override with explicit name if needed:
http.get('/users/123', { name: 'GetSpecificUser' });
```

This auto-naming reduces metric cardinality for APIs with dynamic IDs, making reports cleaner without manual `name` tags.

### Automatic Cookie Handling

Fusillade automatically manages cookies per worker (VU). Cookies received via `Set-Cookie` headers are stored and sent on subsequent requests to matching domains. This enables realistic session-based testing without manual cookie management.

```javascript
// First request - server sets session cookie
http.post('https://api.example.com/login', { user: 'test', pass: 'secret' });

// Subsequent requests automatically include the session cookie
http.get('https://api.example.com/dashboard');  // Cookie header sent automatically
```

### Cookie Manipulation API

For advanced scenarios, you can programmatically manage cookies:

```javascript
// Get the cookie jar
const jar = http.cookieJar();

// Set a cookie manually
jar.set('https://api.example.com', 'session', 'abc123', {
    domain: 'api.example.com',
    path: '/',
    secure: true,
    httpOnly: true,
});

// Get a specific cookie
const session = jar.get('https://api.example.com', 'session');
print(session.value);  // "abc123"

// Get all cookies for a URL
const cookies = jar.cookiesForUrl('https://api.example.com/dashboard');
// Returns array: [{name: 'session', value: 'abc123', ...}, ...]

// Delete a specific cookie
jar.delete('https://api.example.com', 'session');

// Clear all cookies
jar.clear();
```

**Cookie Properties:**
| Property | Type | Description |
|----------|------|-------------|
| `name` | String | Cookie name |
| `value` | String | Cookie value |
| `domain` | String | Cookie domain |
| `path` | String | Cookie path |
| `expires` | Number | Expiration timestamp (Unix epoch) |
| `maxAge` | Number | Max age in seconds |
| `secure` | Boolean | HTTPS only |
| `httpOnly` | Boolean | HTTP only (no JS access) |
| `sameSite` | String | SameSite policy (Strict/Lax/None) |

**Response Cookies:**

Cookies from responses are also accessible via `response.cookies`:

```javascript
const res = http.post('https://api.example.com/login', credentials);

// Access response cookies
const sessionCookie = res.cookies['session'];
if (sessionCookie) {
    print(`Session: ${sessionCookie.value}, expires: ${sessionCookie.expires}`);
}
```

### ws Module

* `ws.connect(url)`: Opens a WebSocket connection, returns a socket object.
* `socket.send(text)`: Sends a text message.
* `socket.recv()`: Blocking receive (returns text, binary as string, or null on close).
* `socket.close()`: Closes the connection.

```javascript
const socket = ws.connect('wss://echo.websocket.org');
socket.send('Hello');
const response = socket.recv();
print(response);
socket.close();
```

### sse Module

*   `sse.connect(url)`: Opens a Server-Sent Events connection.
*   `client.recv()`: Blocking receive (returns `{ event, data, id }` or `null`).
*   `client.close()`: Closes the stream.
*   `client.url`: Property containing the connection URL.

### check, sleep & Utility Functions

* `check(val, checks)` / `assertion(val, checks)`: Verifies boolean conditions against a value. Each check function receives the value and returns true/false. Failed checks are recorded as metrics. On browser tests, screenshots are auto-captured on failure.

```javascript
check(response, {
    'status is 200': (r) => r.status === 200,
    'body contains success': (r) => r.body.includes('success'),
});
```

**Custom Failure Messages:**

Check functions can return a string instead of a boolean to provide custom failure messages:

```javascript
check(response, {
    'status is 200': (r) => r.status === 200 || `Expected 200, got ${r.status}`,
    'has valid data': (r) => {
        const data = r.json();
        if (!data.items) return 'Missing items array';
        if (data.items.length === 0) return 'Items array is empty';
        return true;
    },
});
```

- Return `true` = check passed
- Return `false` = check failed (generic message)
- Return `string` = check failed with custom message (the returned string)

* `sleep(seconds)`: Pauses the virtual user for the specified duration (fractional seconds supported). Sleep time is automatically excluded from iteration response time metrics.
* `sleepRandom(min, max)`: Pauses for a random duration between `min` and `max` seconds. Useful for realistic think times. Sleep time is automatically excluded from iteration response time metrics.
* `print(message)`: Logs a message to stdout and the worker logs file.
* `open(path)`: Reads a file from the local filesystem and returns its content as a string. Useful for loading data files (e.g., JSON or CSV). Only available during initialization, not inside the default function.

### segment (Request Grouping)

* `segment(name, fn)`: Groups requests under a named category for cleaner metrics reporting. Nested segments create hierarchical names (e.g., `Login::Dashboard`).

```javascript
segment('Login Flow', () => {
    http.post('/login', credentials);
    segment('Dashboard', () => {
        http.get('/dashboard');  // Metric: "Login Flow::Dashboard::/dashboard"
    });
});
```

### Console API

Fusillade provides a `console` object with log levels for structured logging:

* `console.log(msg)`: Log at INFO level (default)
* `console.info(msg)`: Log at INFO level
* `console.warn(msg)`: Log at WARN level
* `console.error(msg)`: Log at ERROR level
* `console.debug(msg)`: Log at DEBUG level (hidden by default)
* `console.table(array)`: Display array of objects as a table

```javascript
console.debug('Detailed debugging info');  // Only shown with --log-level debug
console.log('Normal info message');
console.warn('Warning: rate limit approaching');
console.error('Error: request failed');

// Pretty-print tabular data
console.table([
    { name: 'Alice', score: 95 },
    { name: 'Bob', score: 87 },
]);
```

**Log Level CLI Flag:**

```bash
fusillade run test.js --log-level debug   # Show all logs including debug
fusillade run test.js --log-level warn    # Only show warn and error
fusillade run test.js --log-level error   # Only show errors
```

### Environment & Context

* `__ENV`: Object containing all environment variables. Access via `__ENV.API_KEY`.
* `__WORKER_ID`: The current worker's numeric ID (0-indexed). Useful for partitioning data across workers.
* `__SCENARIO`: Name of the currently executing scenario (in multi-scenario tests).
* `__ITERATION`: The current iteration number for this worker (0-indexed).
* `__VU_STATE`: Per-VU state object that persists across iterations. Use for storing session data, counters, or any state that should survive between iterations.

```javascript
// Use worker ID to partition test data
const userId = users[__WORKER_ID % users.length];

// Access environment variables
const apiKey = __ENV.API_KEY || 'default-key';

// Track per-VU state across iterations
__VU_STATE.loginCount = (__VU_STATE.loginCount || 0) + 1;
console.log(`VU ${__WORKER_ID} iteration ${__ITERATION}, logins: ${__VU_STATE.loginCount}`);

// Store session data that persists
if (!__VU_STATE.token) {
    const res = http.post('/login', credentials);
    __VU_STATE.token = res.json().token;
}
// Use cached token in subsequent iterations
http.get('/api/data', { headers: { 'Authorization': `Bearer ${__VU_STATE.token}` } });
```

**Automatic `.env` File Loading:**

Fusillade automatically loads `.env` files when running tests. It checks:
1. The script's directory first
2. The current working directory

```bash
# .env file format
API_KEY=your-secret-key
BASE_URL=https://api.example.com
DEBUG=true
```

Environment variables set in the shell take precedence over `.env` file values.

### Unit Testing

Fusillade provides a built-in unit testing framework for verifying logic before running load tests.

* `describe(name, fn)`: Groups related tests.
* `test(name, fn)`: Defines a test case.
* `expect(value)`: Starts an assertion.
    * `.toBe(expected)`: Strict equality check.
    * `.toEqual(expected)`: Deep equality check (via JSON).
    * `.toBeTruthy()`: Checks if value is truthy.

```javascript
describe("Cart Logic", () => {
    test("calculates total correctly", () => {
        const total = calculateTotal(100, 2);
        expect(total).toBe(200);
    });
});
```

### Protocol Classes

**gRPC:**

* `new GrpcClient()`: Create a new gRPC client.
* `.load(files, includes)`: Loads Protobuf definitions from `.proto` files.
* `.connect(url)`: Connects to gRPC server.
* `.invoke(method, payload)`: Performs a unary RPC call. Method format: `package.Service/Method`.
* `.serverStream(method, request)`: Start a server streaming RPC, returns a `GrpcServerStream`.
* `.clientStream(method)`: Start a client streaming RPC, returns a `GrpcClientStream`.
* `.bidiStream(method)`: Start a bidirectional streaming RPC, returns a `GrpcBidiStream`.

**GrpcServerStream:**
* `.recv()`: Receive next message from server (blocking). Returns object or `null` when stream ends.
* `.close()`: Close the stream early.

**GrpcClientStream:**
* `.send(msg)`: Send a message to the server.
* `.closeAndRecv()`: Close the send side and wait for the server's response.

**GrpcBidiStream:**
* `.send(msg)`: Send a message to the server.
* `.recv()`: Receive a message from the server (blocking). Returns object or `null`.
* `.close()`: Close the stream.

```javascript
// Unary RPC
const client = new GrpcClient();
client.load(['./protos/hello.proto'], ['./protos']);
client.connect('http://localhost:50051');
const response = client.invoke('helloworld.Greeter/SayHello', { name: 'World' });
print(response.message);

// Server Streaming RPC
const stream = client.serverStream('pkg.Service/StreamData', { query: 'test' });
let msg;
while ((msg = stream.recv()) !== null) {
    print(msg.data);
}

// Client Streaming RPC
const stream = client.clientStream('pkg.Service/UploadData');
stream.send({ chunk: 'data1' });
stream.send({ chunk: 'data2' });
const response = stream.closeAndRecv();
print(response.summary);

// Bidirectional Streaming RPC
const stream = client.bidiStream('pkg.Service/Chat');
stream.send({ message: 'Hello' });
const reply = stream.recv();
print(reply.response);
stream.close();
```

**MQTT:**

* `new JsMqttClient()`: Create a new MQTT client.
* `.connect(host, port, clientId)`: Connect to MQTT broker.
* `.subscribe(topic)`: Subscribe to a topic pattern (supports MQTT wildcards `+` and `#`).
* `.publish(topic, payload)`: Publish a message to a topic.
* `.recv()`: Receive next message (blocking). Returns `{ topic, payload, qos }` or `null` on timeout.
* `.close()`: Close the connection.

```javascript
// Publish messages
const mqtt = new JsMqttClient();
mqtt.connect('localhost', 1883, 'test-client');
mqtt.publish('sensors/temp', '22.5');
mqtt.close();

// Subscribe and receive messages
const mqtt = new JsMqttClient();
mqtt.connect('localhost', 1883, 'subscriber');
mqtt.subscribe('sensors/+/temperature');  // Wildcard subscription

let msg;
while ((msg = mqtt.recv()) !== null) {
    print(`${msg.topic}: ${msg.payload}`);
}
mqtt.close();
```

**AMQP (RabbitMQ):**

* `new JsAmqpClient()`: Create a new AMQP client.
* `.connect(url)`: Connect to AMQP broker (e.g., `amqp://localhost`).
* `.subscribe(queue)`: Declare a queue and start consuming messages.
* `.publish(exchange, routingKey, payload)`: Publish a message.
* `.recv()`: Receive next message (blocking). Returns `{ body, deliveryTag }` or `null` on timeout.
* `.ack(deliveryTag)`: Acknowledge a message by its delivery tag.
* `.nack(deliveryTag, requeue)`: Negative acknowledge (reject) a message.
* `.close()`: Close the connection.

```javascript
// Publish messages
const amqp = new JsAmqpClient();
amqp.connect('amqp://localhost');
amqp.publish('', 'my-queue', JSON.stringify({ event: 'order.created' }));
amqp.close();

// Consume messages
const amqp = new JsAmqpClient();
amqp.connect('amqp://guest:guest@localhost:5672');
amqp.subscribe('my-queue');

const msg = amqp.recv();
if (msg !== null) {
    print(msg.body);
    amqp.ack(msg.deliveryTag);
}
amqp.close();
```

### Standard Library

Fusillade provides built-in globals for common operations:

**crypto** - Hashing and HMAC:
* `crypto.md5(data)`: Returns MD5 hash as hex string.
* `crypto.sha1(data)`: Returns SHA1 hash as hex string.
* `crypto.sha256(data)`: Returns SHA256 hash as hex string.
* `crypto.hmac(algorithm, key, data)`: Returns HMAC using md5/sha1/sha256.

**encoding** - Base64:
* `encoding.b64encode(data)`: Encode string to base64.
* `encoding.b64decode(data)`: Decode base64 to string.

**utils** - Random data generation:
* `utils.uuid()`: Generate a UUID v4 string.
* `utils.randomInt(min, max)`: Random integer in range [min, max].
* `utils.randomString(length)`: Random alphanumeric string.
* `utils.randomItem(array)`: Pick a random element from an array.
* `utils.randomEmail()`: Generate a random email address (e.g., `"abc123@test.com"`).
* `utils.randomPhone()`: Generate a random US phone number (e.g., `"+1-555-123-4567"`).
* `utils.randomName()`: Generate a random name object with `first`, `last`, and `full` properties.
* `utils.randomDate(startYear, endYear)`: Generate a random date string in YYYY-MM-DD format.
* `utils.sequentialId()`: Generate a unique sequential ID (unique across workers).

```javascript
// Examples
const hash = crypto.sha256('password');
const token = encoding.b64encode('user:pass');
const id = utils.uuid();
const num = utils.randomInt(1, 100);
const user = utils.randomItem(users);

// Data generation for signup tests
const email = utils.randomEmail();         // "xk7f2b1q@test.com"
const phone = utils.randomPhone();         // "+1-555-847-2901"
const name = utils.randomName();           // { first: "John", last: "Smith", full: "John Smith" }
const dob = utils.randomDate(1970, 2000);  // "1985-07-23"
const orderId = utils.sequentialId();      // Unique ID per worker, e.g., 1000000001
```

### SharedArray

Efficiently share large read-only datasets (like user credentials) across all workers without memory duplication.

```javascript
const users = new SharedArray('users', () => JSON.parse(open('./data/users.json')));
```

### SharedCSV

Load CSV files efficiently with automatic header parsing. Data is shared across all workers.

```javascript
const users = new SharedCSV('./data/users.csv');

export default function() {
    // Get row by index (returns object with column names as keys)
    const user = users.get(__WORKER_ID % users.length);
    http.post('/login', JSON.stringify({ email: user.email, password: user.password }));

    // Get random row
    const randomUser = users.random();
}
```

**Properties & Methods:**
* `length`: Number of rows (excluding header)
* `headers`: Array of column names
* `get(index)`: Get row by index as object
* `random()`: Get random row as object

**Example CSV file (`users.csv`):**
```
email,password,name
user1@example.com,pass123,John
user2@example.com,pass456,Jane
```

### Browser Automation

Fusillade includes native support for headless browser automation via Chromium. This allows for end-to-end testing, including DOM interaction and rendering performance.

* `chromium.launch()`: Launches a new headless browser instance.
* `browser.newPage()`: Opens a new tab/page.
* `browser.close()`: Closes the browser.
* `page.goto(url)`: Navigates to a URL and waits for load.
* `page.content()`: Returns the HTML content of the page.
* `page.click(selector)`: Clicks an element matching the CSS selector.
* `page.type(selector, text)`: Types text into an input element.
* `page.evaluate(script)`: Executes JavaScript in the page context and returns the result.
* `page.metrics()`: Returns performance timing metrics (navigationStart, domInteractive, domComplete, loadEventEnd).
* `page.screenshot()`: Captures a PNG screenshot of the page.

```javascript
export default function() {
    const browser = chromium.launch();
    const page = browser.newPage();

    // Navigate and interact
    page.goto('https://example.com/login');
    page.type('#username', 'testuser');
    page.type('#password', 'secret');
    page.click('#submit');

    // Get page content
    const html = page.content();

    // Execute JavaScript in page
    const title = page.evaluate('document.title');

    // Get performance metrics
    const perf = page.metrics();
    print(`DOM interactive: ${perf.domInteractive - perf.navigationStart}ms`);

    // Capture screenshot
    const png = page.screenshot();

    browser.close();
}
```

### Standard Metrics
Fusillade automatically collects the following metrics:

* **HTTP Timing:**
  * `http_req_duration`: Total request time.
  * `http_req_blocked`: Time waiting for a slot in the connection pool.
  * `http_req_connecting`: Time establishing TCP connection.
  * `http_req_tls_handshaking`: Time performing TLS handshake.
  * `http_req_sending`: Time sending request body.
  * `http_req_waiting`: Time waiting for first byte (TTFB).
  * `http_req_receiving`: Time receiving response body.

* **Throughput & Data:**
  * `http_reqs`: Total number of HTTP requests.
  * `http_req_failed`: Number of failed requests (non-2xx/3xx or network error).
  * `data_sent`: Total bytes sent to the target.
  * `data_received`: Total bytes received from the target.

* **Execution:**
  * `vus`: Number of active virtual users.
  * `iterations`: Number of completed script iterations.

* **Iteration Timing:**
  * `iteration`: Duration of each iteration **excluding** sleep time. This represents the actual "active" work time (HTTP requests, computation, etc.) and is the primary metric for measuring test performance.
  * `iteration_total`: Duration of each iteration **including** sleep time. This represents the total wall-clock time per iteration and is useful for throughput/pacing analysis.

  **Example:** If an iteration makes two 50ms requests with a 500ms sleep between them:
  * `iteration` will report ~100ms (actual work time)
  * `iteration_total` will report ~600ms (work + sleep)

  This separation allows you to accurately measure response times without think-time/pacing skewing your metrics.

### Custom Metrics

Fusillade allows you to define custom business-level metrics beyond standard HTTP latency. These metrics appear in CLI output, JSON reports, and can be validated with thresholds.

**Available Metric Types:**

* `metrics.histogramAdd(name, value)`: Tracks distribution of values (min, max, avg, p95, p99). Ideal for custom timings.
* `metrics.counterAdd(name, value)`: Cumulative sum. Ideal for counting events.
* `metrics.gaugeSet(name, value)`: Stores the most recent value. Ideal for current state (e.g., queue size).
* `metrics.rateAdd(name, success)`: Tracks success rate as a percentage. Ideal for custom success/failure tracking.

**Example:**

```javascript
export const options = {
    stages: [{ duration: '30s', target: 10 }],
    thresholds: {
        'checkout_duration': ['p95 < 500'],
        'items_sold': ['count > 100'],
        'payment_success': ['rate > 0.99'],
    }
};

export default function () {
    let start = Date.now();
    
    // ... business logic ...
    
    // Record how long the checkout took
    metrics.histogramAdd('checkout_duration', Date.now() - start);
    
    // Count items sold
    metrics.counterAdd('items_sold', 3);
    
    // Track success rate
    metrics.rateAdd('payment_success', true);
    
    // Track current queue depth
    metrics.gaugeSet('queue_depth', 42);
}
```

### Querying Stats Programmatically

Use `stats.get(name)` to query collected statistics during the test. This enables adaptive testing scenarios.

```javascript
export default function() {
    http.get('https://api.example.com/data', { name: 'getData' });

    // Query stats for a named request
    const s = stats.get('getData');
    print(`p95: ${s.p95}ms, avg: ${s.avg}ms, count: ${s.count}`);

    // Adaptive behavior based on performance
    if (s.p95 > 500) {
        print('Warning: p95 latency exceeds 500ms');
    }
}
```

**Stats object properties:**
* `p95` (Number): 95th percentile latency in ms
* `p99` (Number): 99th percentile latency in ms
* `avg` (Number): Average latency in ms
* `count` (Number): Total request count
* `min` (Number): Minimum latency in ms
* `max` (Number): Maximum latency in ms

---

## 7. Directory Structure

### Source Code (src/)

* `src/main.rs`: Entry point.
* `src/cli/`: Arg parsing, config loading, HAR conversion.
* `src/engine/`: Core event loop. Manages OS threads and JS Contexts.
* `src/bridge/`: FFI layer exposing Rust functions (http, ws, grpc) to JS.
* `src/stats/`: Metric aggregation, histogram calculation, and reporting.

### User Space

* `scenarios/`: JavaScript test flows.
* `config/`: YAML/JSON run configurations.
* `support/`: Shared JS helpers and libraries.

---

## 8. CLI Reference

### `fusillade run`
Executes a load test script.

**Arguments:**
* `<SCENARIO>`: Path to the JavaScript scenario file.

**Options:**
* `-w, --workers <NUM>`: Override the number of concurrent workers (VUs).
* `-d, --duration <DURATION>`: Override the test duration (e.g., `30s`, `5m`).
* `--iterations <NUM>`: Override iterations per worker (per-vu-iterations mode).
* `--warmup <URL>`: Hit this URL before test starts to warm up connection pools.
* `--response-sink`: Discard response bodies to save memory (enables response sink mode).
* `--threshold <EXPR>`: Add a threshold expression (repeatable, e.g., `--threshold 'http_req_duration:p95<500'`).
* `--abort-on-fail`: Abort immediately if any threshold is breached.
* `--log-level <LEVEL>`: Set console log level: `debug`, `info`, `warn`, `error` (default: `info`).
* `--headless`: Run in headless mode (no TUI, suitable for CI/CD).
* `--json`: Output metrics in newline-delimited JSON format to stdout.
* `--export-json <FILE>`: Save the final summary report to a JSON file.
* `--export-html <FILE>`: Save the final summary report to an HTML file.
* `--out <CONFIG>`: Output configuration. Supports:
  * `--out otlp=<URL>`: Export to OTLP endpoint (e.g., `otlp=http://localhost:4318/v1/metrics`)
  * `--out csv=<FILE>`: Export to CSV file
  * `--out junit=<FILE>`: Export to JUnit XML format (for CI/CD integration)
* `--metrics-url <URL>`: Stream real-time metrics to a URL during test execution. Sends periodic JSON payloads with RPS, latency, errors, etc. At test completion, sends a summary to `<URL>/summary` with endpoint breakdown and HTTP status code counts.
* `--metrics-auth <HEADER>`: Authentication header for metrics URL (format: `HeaderName: value`).
* `-i, --interactive`: Enable interactive control mode (pause, resume, ramp workers).
* `--cloud`: Run on Fusillade Cloud instead of locally.
* `--region <REGION>`: Cloud region to run in (default: `us-east-1`).
* `--jitter <DURATION>`: Chaos: Add artificial latency to requests (e.g., `500ms`).
* `--drop <PROBABILITY>`: Chaos: Drop requests with probability 0.0-1.0 (e.g., `0.05`).
* `--estimate-cost [THRESHOLD]`: Run a dry-run to estimate bandwidth costs. Optional threshold in dollars (default: $10).
* `--watch`: Watch script for changes and re-run automatically (development mode). Uses 1 worker and 5s duration by default unless overridden.
* `--no-endpoint-tracking`: Disable per-endpoint (per-URL) metrics tracking. Useful to reduce cardinality when testing URLs with unique IDs.
* `--no-memory-check`: Disable pre-flight memory capacity check (warning only).
* `--memory-safe`: Enable memory-safe mode: throttle worker spawning if memory usage is high.
* `--save-history`: Save test results to local SQLite database (`fusillade_history.db`). View with `fusillade history`.
* `--capture-errors [FILE]`: Capture failed requests to file for later replay. Default: `fusillade-errors.json`.

### `fusillade history`
View test run history from the local database.

**Options:**
* `--show <ID>`: Show detailed report for a specific run ID.
* `-l, --limit <N>`: Number of recent runs to list (default: 10).

**Example:**
```bash
# List recent test runs
fusillade history

# Show details for a specific run
fusillade history --show 5
```

### `fusillade status`
Show version and status information including cloud authentication status.

### `fusillade logout`
Log out from Fusillade Cloud (removes saved credentials).

### `fusillade compare`
Compare two test run summaries to identify performance regressions or improvements.

**Arguments:**
* `<BASELINE>`: Path to baseline JSON summary (e.g., `results-baseline.json`)
* `<CURRENT>`: Path to current JSON summary (e.g., `results-current.json`)

**Example:**
```bash
# Export summaries from two runs
fusillade run test.js --export-json baseline.json
# ... make changes ...
fusillade run test.js --export-json current.json

# Compare the results
fusillade compare baseline.json current.json
```

Output shows percentage change for each metric:
- **Green**: Improvement (lower latency, higher RPS)
- **Red**: Regression (higher latency, lower RPS)

### `fusillade init`
Initialize a new test script with a starter template.

**Options:**
* `-o, --output <FILE>`: Output file path (default: `test.js`).
* `--config`: Also create a `fusillade.yaml` config file.

**Example:**
```bash
fusillade init -o scenarios/my_test.js --config
```

### `fusillade validate`
Validate a script without running it. Checks for JavaScript syntax errors, configuration validity, and import resolution.

**Arguments:**
* `<SCENARIO>`: Path to the JavaScript scenario file.

**Options:**
* `-c, --config <FILE>`: Optional config file to validate alongside the script.

**Example:**
```bash
fusillade validate scenarios/checkout.js --config config/stress.yaml
```

### `fusillade completion`
Generate shell completion scripts for tab-completion support.

**Arguments:**
* `<SHELL>`: Shell to generate completions for (`bash`, `zsh`, `fish`, `powershell`, `elvish`).

**Example:**
```bash
# Bash (add to ~/.bashrc)
fusillade completion bash >> ~/.bashrc

# Zsh (add to ~/.zshrc)
fusillade completion zsh >> ~/.zshrc

# Fish
fusillade completion fish > ~/.config/fish/completions/fusillade.fish
```

### `fusillade types`
Generates TypeScript type definitions (`index.d.ts`) for IDE support.

**Options:**
* `-o, --output <FILE>`: Path to write the definition file (default: stdout).

### `fusillade schema`
Generates a JSON Schema for validating configuration files.

**Options:**
* `-o, --output <FILE>`: Path to write the schema file (default: stdout).

### `fusillade record`
Starts an interactive proxy to record HTTP traffic and generate a Fusillade scenario.

**Options:**
* `-o, --output <FILE>`: Path to save the generated `.js` flow.
* `-p, --port <PORT>`: Port to listen on (default: `8085`).

**Usage:**
1. Run `fusillade record -o flow.js`.
2. Configure your browser or application to use `http://localhost:8085` as an HTTP proxy.
3. Perform the actions you want to load test.
4. Press `Ctrl+C` in the terminal to save the script.

### `fusillade convert`
Converts a HAR (HTTP Archive) file into a Fusillade JavaScript scenario.

**Arguments:**
* `--input <FILE>`: Input .har file path.
* `--output <FILE>`: Output .js file path.

### `fusillade worker`
Starts a worker node for distributed testing.

**Options:**
* `--listen <ADDR>`: Address to bind the worker server (default: `0.0.0.0:8080`).
* `--connect <ADDR>`: Connect to a Controller (Cluster Mode) instead of listening.

### `fusillade controller`
Starts a controller node to orchestrate workers and serves the live dashboard.

**Options:**
* `--listen <ADDR>`: Address to bind the controller server (default: `0.0.0.0:9000`).

**Web Interface:**
The controller serves a professional web interface at `http://<listen-addr>/`:
*   **Home (`/`)**: A landing page introducing Fusillade.
*   **Live Dashboard (`/dashboard`)**: Real-time visualizations of test metrics, including:
    *   Total requests and error rates.
    *   Average and P95 latencies.
    *   Endpoint-specific performance breakdown.

**Historical Reporting:**
Fusillade automatically saves every test run to a local SQLite database (`fusillade_history.db`). This allows for future analysis and performance trend visualization.

**Asset Distribution:**
When running in distributed mode, Fusillade automatically scans your scenario for local imports (`import ... from './module.js'`) and file dependencies (`open('./data.json')`). These assets are bundled and sent to all worker nodes automatically, ensuring workers have everything they need to execute the test without manual file synchronization.

### Kubernetes Deployment

Fusillade can be deployed on Kubernetes for scalable, distributed load testing. The architecture consists of:

- **Controller**: Orchestrates tests, aggregates metrics, serves dashboard (1 replica)
- **Workers**: Execute load tests, connect to controller via gRPC (auto-scaled)

**Quick Start:**

```bash
# Create namespace
kubectl apply -f k8s/namespace.yaml

# Deploy controller and workers
kubectl apply -f k8s/controller.yaml -n fusillade
kubectl apply -f k8s/worker.yaml -n fusillade

# Check status
kubectl get pods -n fusillade

# Access dashboard
kubectl port-forward svc/fusillade-controller 9000:9000 -n fusillade
# Open http://localhost:9000/dashboard
```

**Architecture:**

```
┌─────────────────────────────────────────────────────────────┐
│                    Kubernetes Cluster                        │
│                                                              │
│  ┌──────────────────┐      gRPC (9001)     ┌─────────────┐  │
│  │                  │◄─────────────────────│   Worker    │  │
│  │    Controller    │◄─────────────────────│   Worker    │  │
│  │   (Dashboard)    │◄─────────────────────│   Worker    │  │
│  │     :9000        │      Metrics         │    ...      │  │
│  └────────┬─────────┘                      └──────┬──────┘  │
│           │                                       │         │
│           │ HTTP                          HPA (3-50 pods)   │
│           ▼                                                 │
│    LoadBalancer:80                                          │
└─────────────────────────────────────────────────────────────┘
```

**Manifest Files** (in `k8s/` directory):

| File | Description |
|------|-------------|
| `namespace.yaml` | Creates isolated `fusillade` namespace |
| `controller.yaml` | Controller Deployment + Services (HTTP & gRPC) |
| `worker.yaml` | Worker Deployment + HorizontalPodAutoscaler |

**Worker Auto-Scaling:**

The HPA automatically scales workers from 3 to 50 replicas based on:
- CPU utilization (target: 70%)
- Memory utilization (target: 80%)

Scale-up is aggressive (5 pods per 30s) for rapid load generation.
Scale-down is conservative (2 pods per 60s, 5-minute stabilization) to avoid thrashing.

**Resource Requirements:**

| Component | CPU Request | CPU Limit | Memory Request | Memory Limit |
|-----------|-------------|-----------|----------------|--------------|
| Controller | 250m | 500m | 256Mi | 512Mi |
| Worker | 500m | 2000m | 512Mi | 2Gi |

Adjust worker resources based on your test complexity and target load.

**Controller API Endpoints:**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/` | GET | Controller info page |
| `/dashboard` | GET | Real-time metrics dashboard |
| `/api/stats` | GET | Current test statistics (JSON) |
| `/api/workers` | GET | List connected workers |
| `/api/dispatch` | POST | Dispatch test to all workers |
| `/api/history` | GET | Past test runs (from SQLite) |
| `/api/screenshots` | GET | List saved screenshots |
| `/metrics` | POST | Receive metrics from workers |

**Running a Test:**

Once deployed, workers automatically connect to the controller. To run a test:

1. Port-forward to the controller or use the LoadBalancer IP
2. Check connected workers via `/api/workers`
3. POST your test script to `/api/dispatch`
4. Monitor results on the dashboard at `/dashboard`

```bash
# Check connected workers
curl http://localhost:9000/api/workers
# Returns: {"workers":[{"id":"abc-123","address":"unknown","available_cpus":4}]}

# Dispatch test to all workers
curl -X POST http://localhost:9000/api/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "script_content": "export default function() { http.get(\"https://httpbin.org/get\"); }",
    "config": {
      "vus": 10,
      "duration_secs": 60
    }
  }'
# Returns: {"success":true,"workers_dispatched":3,"message":"Test dispatched to 3 workers"}

# Monitor stats
curl http://localhost:9000/api/stats
```

**Local Testing with Docker Compose:**

For local development and testing, use `docker-compose`:

```bash
# Start controller + 3 workers
docker-compose up --build

# Access dashboard at http://localhost:9000/dashboard
# gRPC port exposed at localhost:9001
```

Or use the test script without Docker:

```bash
# Start controller and workers as local processes
./scripts/test-distributed.sh
```

### `fusillade replay`
Replay failed requests from an errors file for debugging.

**Arguments:**
* `<INPUT>`: Path to `fusillade-errors.json` file.

**Options:**
* `--parallel`: Run requests in parallel instead of sequentially.

### `fusillade export`
Export failed requests to different formats.

**Arguments:**
* `<INPUT>`: Path to `fusillade-errors.json` file.

**Options:**
* `--format <FORMAT>`: Output format (currently: `curl`).
* `-o, --output <FILE>`: Output file (stdout if not specified).

### `fusillade exec`
Execute a JavaScript snippet directly (for debugging).

**Arguments:**
* `<SCRIPT>`: JavaScript code to execute.

---

## 9. Chaos Engineering (Fault Injection)

Fusillade includes native chaos engineering capabilities to test how your system behaves under adverse conditions—without needing external tools like Toxiproxy.

### CLI Flags

```bash
# Add 500ms jitter and drop 5% of requests
fusillade run scenarios/checkout.js --jitter 500ms --drop 0.05
```

### Configuration

These can also be set in your script's `options`:

```javascript
export const options = {
    workers: 10,
    duration: '30s',
    jitter: '500ms',  // Add artificial latency
    drop: 0.05,       // Drop 5% of requests
};
```

### Behavior

* **Jitter (`--jitter`):** Adds artificial delay before each request. Simulates network latency or slow upstreams.
* **Drop (`--drop`):** Randomly fails requests with a "Simulated network drop" error. Simulates packet loss or connection resets.

Dropped requests are recorded as failures with status `0` and error `"Simulated network drop"`.

---

## 10. Cost Estimation

Estimate data transfer costs before running large-scale tests to avoid surprise bandwidth bills.

### Usage

```bash
# Basic cost estimation (default $10 warning threshold)
fusillade run scenarios/heavy_test.js --estimate-cost

# Custom warning threshold ($50)
fusillade run scenarios/heavy_test.js --estimate-cost 50
```

### How It Works

1. Fusillade runs a brief **dry run** (5 iterations, ~3 seconds).
2. Calculates average request rate and response size.
3. Extrapolates to your configured test duration and worker count.
4. Shows estimated data transfer (GB) and AWS egress cost.

### Example Output

```
⚙️  Running cost estimation (dry run)...

📊 Cost Estimation
------------------------------------
Est. Requests:      ~12500
Est. Data Transfer: 2.45 GB
Est. AWS Cost:      ~$0.22 (at $0.09/GB)
------------------------------------
Proceed? [y/N]
```

For tests estimated to cost over $10, a **warning** is displayed instead:

```
WARNING: High Bandwidth Estimate
------------------------------------
Est. Requests:      ~500000
Est. Data Transfer: 125.50 GB
Est. AWS Cost:      ~$11.30 (at $0.09/GB)
------------------------------------
Proceed? [y/N]
```

---

## 11. Smart Replay (Error Debugging)

When requests fail during a load test, Fusillade can capture them for later analysis and replay.

### Capturing Errors

Failed requests are automatically logged to `fusillade-errors.json` (JSONL format - one JSON per line).

### Export to cURL

Convert captured failures to cURL commands for easy sharing with backend developers:

```bash
fusillade export fusillade-errors.json --format curl
```

**Output:**
```bash
# Request 1 - POST https://api.example.com/checkout (Status: 500, Error: Internal Server Error)
curl -X POST \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer token123" \
  -d "{\"item\":\"1234\"}" \
  'https://api.example.com/checkout'
```

### Replay Failed Requests

Re-execute the failed requests to verify if issues are resolved:

```bash
fusillade replay fusillade-errors.json
```

**Options:**
* `--parallel`: Execute requests in parallel (default: sequential with 100ms delay).

### Error File Format

The errors file uses JSONL format for streaming writes:

```json
{"timestamp":"2026-01-07T21:00:00Z","method":"POST","url":"https://api.example.com/users","headers":{"Content-Type":"application/json"},"body":"{\"name\":\"test\"}","status":500,"error":"Internal Server Error","response_body":"Error"}
```

---

## 12. Terminal TUI & Interactive Control

Fusillade includes a built-in terminal user interface (TUI) that displays live statistics and provides interactive controls during load test runs. The TUI is enabled by default when running locally; use `--headless` to disable it for CI/CD pipelines.

### TUI Layout

When you run a test without `--headless`, Fusillade renders a full-screen terminal dashboard:

```
┌─ Fusillade ─────────────────────────────────────────────────────────┐
│ ● RUNNING  │  Workers: 1000  │  Elapsed: 01:23 / 02:00              │
├─────────────────────────────────────────────────────────────────────┤
│  Requests: 12,345    RPS: 423.1    Errors: 12 (0.1%)                │
│  Sent: 15.2 MB       Received: 128.4 MB                             │
│                                                                     │
│  Avg: 23.4ms  Min: 1.2ms  Max: 892ms                                │
│  P50: 18.2ms  P90: 45.1ms  P95: 67.3ms  P99: 234.5ms                │
├─ Status Codes ──────────────────────────────────────────────────────┤
│  200: 12,100  │  404: 120  │  500: 12                               │
├─ Endpoints ─────────────────────────────────────────────────────────┤
│  Name                     Reqs    Avg      P95      Errs            │
│  GET /api/users           4521    12.3ms   34.5ms   2               │
│  POST /api/login          3210    45.2ms   89.3ms   8               │
├─────────────────────────────────────────────────────────────────────┤
│ [p] Pause  [s] Stop  [+/-] Workers ±10  [r] Ramp  [q] Quit          │
└─────────────────────────────────────────────────────────────────────┘
```

The TUI shows:
- **Header**: Test status (RUNNING/PAUSED/STOPPED), current worker count, and elapsed/total time.
- **Stats panel**: Total requests, requests per second (RPS), error count and percentage, data transfer, and latency percentiles (avg, min, max, P50, P90, P95, P99).
- **Status codes**: Color-coded HTTP status code breakdown (green=2xx, yellow=3xx, magenta=4xx, red=5xx).
- **Endpoints table**: Per-endpoint breakdown sorted by request count, showing request count, average latency, P95 latency, and error count. Scrollable with arrow keys.
- **Controls footer**: Available keyboard shortcuts, or input prompt when in ramp/tag mode.

Stats refresh every second.

### TUI Key Bindings

| Key | Action |
|-----|--------|
| `p` | Toggle pause/resume |
| `+` or `=` | Increase workers by 10 |
| `-` | Decrease workers by 10 (minimum 1) |
| `r` | Enter ramp input mode (type target worker count, Enter to submit, Esc to cancel) |
| `t` | Enter tag input mode (type `key=value`, Enter to submit, Esc to cancel) |
| `s` | Stop the test (graceful shutdown) |
| `q` or `Esc` | Quit (stops test and exits) |
| `↑` / `↓` | Scroll the endpoints table |
| `Ctrl+C` | Force quit |

### Running Without the TUI

For CI/CD pipelines, scripts, or environments without a terminal, use `--headless`:

```bash
# No TUI, text output only
fusillade run scenarios/checkout.js --headless

# JSON output for machine consumption
fusillade run scenarios/checkout.js --json
```

Both `--headless` and `--json` disable the TUI.

### Headless Interactive Mode

You can combine `--headless` with `--interactive` to get text-based interactive control via stdin (the original behavior before the TUI was added):

```bash
fusillade run scenarios/checkout.js --headless --interactive -w 10 -d 5m
```

### Interactive Commands (Headless Mode)

When running with `--headless --interactive`, Fusillade accepts commands via stdin:

| Command | Aliases | Description | Example |
|---------|---------|-------------|---------|
| `ramp <N>` | `scale` | Dynamically scales the number of workers to N. Useful for finding breaking points. | `ramp 100` |
| `pause` | | Pauses all worker execution. Active requests complete, but no new iterations start. | `pause` |
| `resume` | `unpause` | Resumes execution after a pause. | `resume` |
| `tag <key>=<val>` | | Injects a custom tag into all subsequent metrics. Useful for marking deployment events or test phases. | `tag env=prod` |
| `status` | `stats` | Prints current test status (workers, RPS, duration). | `status` |
| `stop` | `quit`, `exit` | Initiates a graceful shutdown (waits for `stop` timeout). | `stop` |

> [!NOTE]
> Dynamic scaling (`ramp`) is currently supported in single-scenario configurations only. `pause`, `resume`, `tag`, and `stop` are fully supported in all modes.

---

## 13. CLI Quick Reference

| Command | Description |
|---------|-------------|
| `fusillade run <file>` | Execute a load test |
| `fusillade run <file> -w 50 -d 5m` | Run with 50 workers for 5 minutes |
| `fusillade run <file> -i` | Run with interactive control |
| `fusillade run <file> --jitter 500ms --drop 0.05` | Run with chaos injection |
| `fusillade run <file> --estimate-cost` | Estimate transfer costs first |
| `fusillade init` | Create starter test script |
| `fusillade init -o test.js --config` | Create script and config file |
| `fusillade validate <file>` | Validate script without running |
| `fusillade completion <shell>` | Generate shell completions |
| `fusillade export <errors.json> --format curl` | Export failures to cURL |
| `fusillade replay <errors.json>` | Replay failed requests |
| `fusillade types -o index.d.ts` | Generate TypeScript definitions |
| `fusillade schema -o config.json` | Generate JSON schema |
| `fusillade record -o flow.js` | Record HTTP traffic as a script |
| `fusillade convert --input file.har` | Convert HAR to JS |
| `fusillade worker --listen 0.0.0.0:8080` | Start worker node |
| `fusillade controller --listen 0.0.0.0:9000` | Start controller node |

---

## 14. Testing

### Running Unit Tests

Run the Rust unit tests with:

```bash
cargo test
```

To run tests for a specific module:

```bash
cargo test bridge::http::tests  # HTTP module tests
cargo test stats::tests         # Stats module tests
cargo test bridge::crypto       # Crypto module tests
```

### Integration Test Scenarios

Integration tests are JavaScript scenarios in the `scenarios/` directory:

| File | Description |
|------|-------------|
| `scenarios/test.js` | Basic functionality test |
| `scenarios/test_proto.js` | HTTP protocol version (`proto`) field test |

Run an integration test:

```bash
fusillade run scenarios/test_proto.js
```

### HTTP Protocol Version Tests

The `proto` field on HTTP responses is tested at multiple levels:

**Unit tests** (`src/bridge/http.rs`):
- `test_version_to_proto_http1` - Verifies HTTP/0.9, 1.0, 1.1 all map to `"h1"`
- `test_version_to_proto_http2` - Verifies HTTP/2 maps to `"h2"`
- `test_version_to_proto_http3` - Verifies HTTP/3 maps to `"h3"`
- `test_http_response_default_proto` - Verifies HttpResponse struct with h1 proto
- `test_http_response_h2_proto` - Verifies HttpResponse struct with h2 proto

**Integration test** (`scenarios/test_proto.js`):
```javascript
export default function() {
    let res = http.get('https://httpbin.org/get');

    check(res, {
        'proto field exists': (r) => r.proto !== undefined,
        'proto is h1 or h2': (r) => r.proto === 'h1' || r.proto === 'h2',
    });

    print('Response proto: ' + res.proto);
}
```

---

## Memory-Aware Scaling

Fusillade provides memory-aware features to prevent OOM crashes and help with capacity planning.

### Pre-flight Memory Check

Before starting a test, Fusillade estimates whether the requested worker count can fit in available memory. If it exceeds the estimated capacity, a warning is displayed:

```
╭────────────────────────────────────────────────────────────╮
│ MEMORY WARNING                                              │
├────────────────────────────────────────────────────────────┤
│ Requested workers:  500000                                 │
│ Estimated max:      350000 (based on available RAM)        │
│ Available RAM:      4.2 GB                                 │
│ Estimated needed:   6.0 GB                                 │
├────────────────────────────────────────────────────────────┤
│ The test may run out of memory and crash.                  │
│ Use --no-memory-check to suppress this warning.            │
╰────────────────────────────────────────────────────────────╯
```

The test will still run, but you may encounter OOM errors.

### Memory Estimation

Based on production benchmarks:

| Memory | Estimated Max Workers |
|--------|----------------------|
| 1 GB   | ~84,000              |
| 4 GB   | ~340,000             |
| 8 GB   | ~680,000             |
| 16 GB  | ~1,360,000           |
| 32 GB  | ~2,700,000           |

**Formula:**
- **Base overhead:** ~30 MB (engine, runtime, connections)
- **Per-worker:** ~12 KB (thread stack, JS context, state)
- `max_workers = (available_ram - 30MB) / 12KB`

### CLI Flags

| Flag | Description |
|------|-------------|
| `--no-memory-check` | Disable the pre-flight memory warning |
| `--memory-safe` | Enable memory-safe mode (throttle spawning if memory is high) |

**Examples:**

```bash
# Normal run (warning shows if needed)
fusillade run script.js -w 500000 -d 30s

# Suppress warning (you know what you're doing)
fusillade run script.js -w 500000 -d 30s --no-memory-check

# Memory-safe mode (throttle if memory gets high)
fusillade run script.js -w 100000 -d 5m --memory-safe
```

### Reducing Memory Usage

If you're hitting memory limits, consider:

1. **Enable response sink mode:** Discard response bodies to save memory
   ```javascript
   export const options = {
       workers: 10000,
       duration: '1m',
       response_sink: true,  // Don't store response bodies
   };
   ```

2. **Increase stack size if needed:** Default is 32KB per worker. Increase for complex scripts:
   ```javascript
   export const options = {
       workers: 10000,
       duration: '1m',
       stack_size: 65536,  // 64KB per worker for complex scripts
   };
   ```

3. **Use distributed mode:** Spread workers across multiple machines
   ```bash
   # On worker nodes
   fusillade worker --listen 0.0.0.0:8080
   
   # On controller
   fusillade run script.js --execution distributed --workers node1:8080,node2:8080
   ```

