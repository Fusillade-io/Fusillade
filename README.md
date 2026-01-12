# Fusillade 

**High-performance load testing engine written in Rust. JavaScript scripting. Production-ready.**

Fusillade is a modern load testing platform that combines the raw speed of **Rust** with the developer-friendly scripting of **JavaScript**.

## Quick Start

```bash
# Install
cargo install --path .

# Run a test
fusillade run scenarios/test.js

# Or use the short alias
fusi run scenarios/test.js
```

## Example Test

```javascript
export const options = {
    workers: 100,
    duration: '30s',
    thresholds: {
        'http_req_duration': ['p95 < 500'],
        'http_req_failed': ['rate < 0.01'],
    }
};

export default function () {
    const res = http.post('https://api.example.com/login', JSON.stringify({
        username: 'user',
        password: 'pass'
    }));
    
    check(res, {
        'status is 200': (r) => r.status === 200,
    });
    
    sleep(1);
}
```

## Performance

- **100,000+ RPS** on a single machine
- **Sub-millisecond latency** (0.1-0.2ms average)
- **Low memory footprint** with Rust's zero-cost abstractions

## Features

### Protocols
- **HTTP/2** with connection pooling and multiplexing
- **WebSockets** (TLS support)
- **gRPC** (dynamic protobuf reflection)
- **MQTT** & **AMQP** (IoT/messaging)
- **SSE** (Server-Sent Events)

### Execution Modes
```bash
# Local run
fusillade run test.js

# CI/CD mode (headless + JUnit report)
fusillade run test.js --headless --out junit=results.xml

# CSV export
fusillade run test.js --out csv=metrics.csv

# Distributed mode
fusillade run test.js --execution distributed --workers host1:8000,host2:8000
```

### HAR Import
```bash
fusillade convert --input recording.har --output flow.js
```

### Configuration Options
- `workers` - Number of virtual users
- `duration` - Test duration
- `stages` - Ramping schedule
- `thresholds` - Pass/fail criteria
- `scenarios` - Multiple concurrent user flows

## Documentation

See [documentation.md](documentation.md) for the complete API reference.

## Technology

- **Runtime**: Rust + QuickJS (JavaScript engine)
- **Concurrency**: OS thread-per-worker model
- **Memory**: RAII-based, no garbage collector overhead
- **Binary Size**: ~35MB

## License

GNU General Public License v3.0
