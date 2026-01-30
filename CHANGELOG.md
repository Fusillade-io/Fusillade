# Changelog

All notable changes to Fusillade are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.3.1] - 2026-01-30

### Added
- `--dry-run` flag to preview execution plan without running the test
- `--log-filter scenario=<name>` flag to filter log output to a specific scenario
- WebSocket auto-reconnect with exponential backoff (`reconnect` and `maxRetries` options)
- `socket.isConnected()` method for checking WebSocket connection status
- Browser `page.setContent(html)` to set page HTML content
- Browser `page.focus(selector)` to focus an element
- Browser `page.select(selector, values)` to select options in a `<select>` element
- Browser `page.getCookies()` to retrieve cookies as an array
- Browser `page.setCookie(cookie)` to set a cookie with name, value, domain, path, secure, maxAge
- Browser `page.deleteCookie(name)` to delete a cookie by name
- Browser `page.queryAll(selector)` to query all matching elements and return their tag, text, id, className
- Browser `page.waitForResponse(urlPattern, [timeoutMs])` to wait for a network response matching a URL pattern
- AMQP `client.declareExchange(name, type, [opts])` for declaring exchanges
- AMQP `client.declareQueue(name, [opts])` for declaring queues with durable, autoDelete, exclusive options
- AMQP `client.bindQueue(queue, exchange, routingKey)` for binding queues to exchanges
- MQTT `client.unsubscribe(topic)` to unsubscribe from topics
- MQTT `publish` now supports optional `retain` parameter
- `utils.randomAddress()` generating random street, city, state, zip, and full address
- `http.graphql(url, query, [variables])` convenience method for GraphQL requests
- `http.batch()` now supports optional `onProgress` callback
- `res.matchesSchema(schema)` for JSON schema type validation on responses
- `stats.get(name)` for accessing runtime metric values from scripts
- Connection pool metrics: automatic tracking of `pool_hits` and `pool_misses` in reports, JSON, and HTML exports
- OpenAPI/Swagger import: `fusillade convert` now accepts `.yaml`/`.yml`/`.json` OpenAPI specs and generates test scripts
- FormData documentation section in DOCUMENTATION.md
- `fusillade login` and `fusillade whoami` CLI documentation

### Changed
- `fusillade convert` now auto-detects file format (HAR vs OpenAPI) for JSON inputs

## [1.3.0] - 2026-01-29

### Added
- `res.html(selector)` method for CSS selector-based HTML parsing on HTTP responses
- `socket.sendBinary(data)` for sending binary WebSocket frames via base64-encoded strings
- Optional QoS parameter for MQTT `subscribe(topic, [qos])` and `publish(topic, payload, [qos])`
- Dedicated Crypto & Encoding documentation page on fusillade.io

### Fixed
- HAR conversion generating `sleep(100)` (100 seconds) instead of `sleep(0.1)` (100ms)
- `fusillade convert` command documentation: input file is a positional argument, not `--input`
- Removed non-existent `preAllocatedWorkers` and `maxWorkers` from arrival-rate docs
- Fixed incorrect `vus = workers` config alias documentation (only valid in scenarios)
- Added missing config aliases to docs: `noEndpointTracking`, `abortOnFail`, `memorySafe`, `minIterationDuration`

## [1.2.0] - 2026-01-29

### Added
- Browser Page API methods: `title()`, `url()`, `fill(selector, value)`, `waitForSelector(selector, timeout)`, `waitForNavigation(timeout)`, `waitForTimeout(ms)`, `close()`
- Data transfer limits and overage billing support

### Changed
- Renamed all VU/virtual user terminology to "workers" across codebase and documentation

## [1.1.0] - 2026-01-29

### Added
- Browser automation documentation
- StatsD metrics export

## [1.0.3] - 2026-01-29

### Changed
- Terminology updates throughout codebase and documentation

## [1.0.2] - 2026-01-28

### Added
- `--insecure` CLI flag to skip TLS certificate verification (for self-signed certs)
- `--max-redirects` CLI flag to control HTTP redirect behavior
- `--user-agent` CLI flag to set default User-Agent header
- `--out statsd=<HOST:PORT>` for exporting metrics to StatsD/Datadog/Graphite
- Custom metrics now support optional `tags` parameter for fine-grained filtering
- Protocol metrics tests for WebSocket, gRPC, MQTT, AMQP, SSE, and Browser modules

### Fixed
- Documentation updated to show custom metrics tags parameter

## [1.0.1] - 2026-01-28

### Fixed
- SharedIterations executor now properly shares iterations across all VUs using atomic counter
- capture_errors CLI flag now properly captures failed HTTP requests to JSON file for replay
- Duration parsing now returns proper errors instead of panicking on invalid input

### Added
- Scenario name validation (alphanumeric, underscore, hyphen, dot only; max 64 chars)
- Dropped metrics tracking infrastructure (`get_dropped_metrics_count()`, `reset_dropped_metrics_count()`)
- Internal memory safety documentation

### Changed
- HTTP error paths now use centralized error response creation with automatic capture
- Console logging functions now track dropped metrics when channel disconnects

## [1.0.0] - 2026-01-28

Major release with comprehensive load testing features.

### Added

#### Executor Types
- constant-arrival-rate executor with timer-based request dispatch
- ramping-arrival-rate executor with variable RPS over time
- ramping-vus executor with dynamic VU scaling
- per-vu-iterations executor for fixed iterations per VU
- shared-iterations executor for shared iteration pool across VUs
- dropped_iterations metric for arrival rate executors

#### Cookie Manipulation API
- `http.cookieJar()` for programmatic cookie management
- JsCookieJar class with set/get/cookiesForUrl/clear/delete methods
- `response.cookies` object for accessing Set-Cookie headers

#### Check Improvements
- Custom failure messages (return string instead of boolean)
- `handleSummary()` hook for custom output generation

#### HTTP Improvements
- FormData class for multipart file uploads
- Error differentiation with `error` type and `errorCode` fields
- `statusText` field on response objects
- `http.addHook()` and `http.clearHooks()` for request/response hooks
- `retryOn` and `retryDelayFn` for custom retry logic
- Automatic URL normalization (replaces IDs with `:id` placeholder)

#### Console API
- `console.log/info/warn/error/debug` with log level filtering
- `console.table` for tabular data display
- `--log-level` CLI flag (debug, info, warn, error)

#### VU State
- `__VU_STATE` object persists across iterations within a VU
- `__ITERATION` global exposes current iteration count

#### CLI Enhancements
- `--iterations` flag for per-vu-iterations mode
- `--warmup` flag for connection pool warmup
- `--response-sink` flag to discard response bodies (memory optimization)
- `--threshold` flag (repeatable) for CLI-specified thresholds
- `--abort-on-fail` flag for fast CI/CD failure on threshold breach

### Changed
- Unified HttpResponse and SyncHttpResponse shapes
- `hasHeader()` now accepts optional value parameter for matching

## [0.9.0] - 2026-01-28

### Added
- Comprehensive audit improvements
- RPS pause calculation fixes
- TUI guard for terminal safety
- Graceful stop handling
- Error differentiation in HTTP responses

### Fixed
- Pause timer and duration tracking in TUI and engine
- Terminal TUI stability issues

## [0.8.2] - 2026-01-27

### Added
- Terminal TUI with live stats display
- Interactive controls (pause/resume, ramp workers, stop)
- Real-time metrics visualization

## [0.8.1] - 2026-01-26

### Added
- Data transfer metrics (bytes sent/received) for cloud reporting

## [0.8.0] - 2026-01-25

### Added
- gRPC streaming support (client/server/bidirectional)
- MQTT streaming with QoS levels
- AMQP message streaming

## [0.7.4] - 2026-01-24

### Added
- `--config` CLI flag for external configuration files
- `--parallel` flag for replay command

### Fixed
- Replay command parallel execution

## [0.7.3] - 2026-01-23

### Changed
- Distributed worker improvements
- Internal cleanup and refactoring

## [0.7.2] - 2026-01-22

### Added
- Unit tests for distributed and http_client modules
- Community health files (Code of Conduct, Security Policy)

## [0.7.0] - 2026-01-20

### Added
- History command for test run history
- HTML report generation
- Comprehensive test coverage improvements

## [0.6.0] - 2026-01-15

### Added
- MQTT protocol support
- AMQP/RabbitMQ protocol support
- SSE (Server-Sent Events) support
- Browser automation with Chromium

## [0.5.0] - 2026-01-01

Initial public release.

### Added
- HTTP/1.1 and HTTP/2 support with connection pooling
- WebSocket support
- gRPC support (unary calls)
- JavaScript test scripts via QuickJS
- Multi-scenario support
- Threshold validation
- JSON/CSV/HTML reporting
- OpenTelemetry metrics export
- Distributed mode (controller/worker)
- CLI with run, validate, record, replay commands
