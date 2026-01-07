[![Crates.io](https://img.shields.io/crates/v/postgres-notify.svg)](https://crates.io/crates/postgres-notify)
[![Documentation](https://docs.rs/postgres-notify/badge.svg)](https://docs.rs/postgres-notify)
[![Maintenance](https://img.shields.io/badge/maintenance-actively--developed-brightgreen.svg)](https://github.com/macprog-guy/postgres-notify)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# postgres-notify

A resilient PostgreSQL client wrapper for `tokio_postgres` with automatic reconnection, query timeouts, and NOTIFY/LISTEN support.

- Automatic reconnection with exponential backoff and jitter
- Query timeouts with server-side cancellation
- PostgreSQL NOTIFY/LISTEN pub/sub notifications
- RAISE message capture for execution logging

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
postgres-notify = "0.3"
tokio = { version = "1", features = ["rt", "time"] }
tokio-postgres = "0.7"
```

### Feature Flags

| Feature   | Default | Description |
|-----------|---------|-------------|
| `chrono`  | Yes     | Use `DateTime<Utc>` for timestamps instead of `SystemTime` |
| `serde`   | Yes     | Enable serialization for message types |
| `tracing` | Yes     | Structured logging via the `tracing` crate |

To disable default features:

```toml
[dependencies]
postgres-notify = { version = "0.3", default-features = false }
```

## Quick Start

```rust
use postgres_notify::{PGRobustClient, PGRobustClientConfig, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = PGRobustClientConfig::new("postgres://localhost/mydb", NoTls)
        .callback(|msg: PGMessage| println!("{}", msg));

    let mut client = PGRobustClient::spawn(config).await?;

    let rows = client.query("SELECT $1::TEXT", &[&"hello"], Some(Duration::from_secs(5))).await?;
    println!("Result: {}", rows[0].get::<_, String>(0));

    Ok(())
}
```

## Features

### LISTEN/NOTIFY

PostgreSQL supports asynchronous notifications via LISTEN/NOTIFY commands. This allows the database to push messages to clients without polling.

```rust
use postgres_notify::{PGRobustClient, PGRobustClientConfig, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = PGRobustClientConfig::new("postgres://localhost/mydb", NoTls)
        .callback(|msg: PGMessage| {
            if let PGMessage::Notify(notification) = msg {
                println!("Channel: {}, Payload: {}", notification.channel, notification.payload);
            }
        });

    let mut client = PGRobustClient::spawn(config).await?;

    // Subscribe to channels
    client.subscribe_notify(&["events", "updates"], Some(Duration::from_secs(5))).await?;

    // Subscriptions are automatically restored after reconnection
    // Unsubscribe when done
    client.unsubscribe_notify(&["events"], None).await?;

    Ok(())
}
```

### RAISE/Logs

Capture PostgreSQL RAISE messages during query execution. Useful for debugging stored procedures and functions.

```rust
use postgres_notify::{PGRobustClient, PGRobustClientConfig, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = PGRobustClientConfig::new("postgres://localhost/mydb", NoTls)
        .callback(|_| {});

    let mut client = PGRobustClient::spawn(config).await?;

    // Capture logs from a query
    let (result, logs) = client.with_captured_log(async |c| {
        c.simple_query(
            "DO $$ BEGIN RAISE NOTICE 'Processing started'; END; $$",
            Some(Duration::from_secs(5))
        ).await?;
        Ok(())
    }).await?;

    for log in logs {
        println!("{}", log);
    }

    Ok(())
}
```

**Log Levels**: DEBUG, LOG, INFO, NOTICE, WARNING, ERROR, FATAL, PANIC

### Automatic Reconnection

If the connection is lost, the client automatically reconnects with exponential backoff:

- Initial delay: 500ms
- Doubles after each attempt (with jitter)
- Maximum delay: 1 hour
- Default max attempts: 10

On reconnection:
- Connect script is re-executed
- Channel subscriptions are restored

```rust
let config = PGRobustClientConfig::new(url, NoTls)
    .max_reconnect_attempts(5)  // Limit retry attempts
    .callback(|msg| {
        match msg {
            PGMessage::Reconnect { attempts, max_attempts, .. } => {
                println!("Reconnecting: attempt {} of {}", attempts, max_attempts);
            }
            PGMessage::Connected { .. } => println!("Connected!"),
            PGMessage::FailedToReconnect { .. } => println!("Gave up reconnecting"),
            _ => {}
        }
    });
```

### Query Timeouts

All query methods accept an optional timeout. If exceeded, the query is cancelled server-side.

```rust
use std::time::Duration;

// With explicit timeout
let rows = client.query("SELECT pg_sleep(10)", &[], Some(Duration::from_secs(1))).await;
// Returns Err(PGError::Timeout(..))

// Use default timeout (1 hour)
let rows = client.query("SELECT 1", &[], None).await?;

// Configure default timeout
let config = PGRobustClientConfig::new(url, NoTls)
    .default_timeout(Duration::from_secs(30));

// Manual cancellation
client.cancel_query().await?;
```

## Configuration

### Builder Pattern

```rust
use postgres_notify::{PGRobustClientConfig, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

let config = PGRobustClientConfig::new("postgres://user:pass@localhost/db", NoTls)
    // Event callback (required for NOTIFY/RAISE)
    .callback(|msg: PGMessage| println!("{}", msg))

    // Reconnection settings
    .max_reconnect_attempts(10)

    // Default query timeout
    .default_timeout(Duration::from_secs(300))

    // SQL executed on each connection
    .connect_script("SET timezone = 'UTC'; SET statement_timeout = '30s';")

    // PostgreSQL application name
    .application_name("my-service")

    // Pre-configure LISTEN channels
    .subscriptions(["events", "updates"]);
```

## API Overview

### Main Types

| Type | Description |
|------|-------------|
| `PGRobustClient<TLS>` | Main client with resilience features |
| `PGRobustClientConfig<TLS>` | Builder for client configuration |
| `PGMessage` | Event enum delivered to callbacks |
| `PGError` | Error type with timeout/reconnect variants |
| `PGResult<T>` | Alias for `Result<T, PGError>` |

### Query Methods

All methods mirror `tokio_postgres::Client` with an additional timeout parameter:

| Method | Description |
|--------|-------------|
| `query` | Execute query, return all rows |
| `query_one` | Execute query, return exactly one row |
| `query_opt` | Execute query, return zero or one row |
| `query_raw` | Execute query with streaming results |
| `execute_raw` | Execute statement, return affected row count |
| `prepare` | Prepare a statement |
| `transaction` | Begin a transaction |
| `simple_query` | Execute simple (non-parameterized) query |
| `batch_execute` | Execute multiple statements |

### Message Types

```rust
pub enum PGMessage {
    Notify(PGNotifyMessage),     // NOTIFY received
    Raise(PGRaiseMessage),       // RAISE message received
    Reconnect { .. },            // Reconnection attempt
    Connected { .. },            // Connection established
    Timeout { .. },              // Query timeout occurred
    Cancelled { .. },            // Query cancellation result
    FailedToReconnect { .. },    // Max reconnect attempts reached
    Disconnected { .. },         // Connection lost
}
```

## Error Handling

```rust
use postgres_notify::{PGError, PGResult};

async fn example(client: &mut PGRobustClient<NoTls>) -> PGResult<()> {
    match client.query("SELECT 1", &[], None).await {
        Ok(rows) => println!("Got {} rows", rows.len()),
        Err(PGError::Timeout(duration)) => {
            println!("Query timed out after {:?}", duration);
        }
        Err(PGError::FailedToReconnect(attempts)) => {
            println!("Failed to reconnect after {} attempts", attempts);
        }
        Err(PGError::Postgres(e)) => {
            if e.code().map(|c| c.code().starts_with("23")).unwrap_or(false) {
                println!("Constraint violation: {}", e);
            }
        }
        Err(e) => return Err(e),
    }
    Ok(())
}
```

## Best Practices

### Callback Safety

The callback runs in a background tokio task. If it panics:

- The internal message log becomes inaccessible
- The connection polling task terminates

**Recommendation**: Never panic in callbacks. Use `std::panic::catch_unwind` if calling untrusted code.

### Security

The `connect_script` and `application_name` values are interpolated directly into SQL. **Do not pass untrusted user input** to these methods.

### Performance

- This is a single-connection client, not a connection pool
- `query`, `query_one`, and `query_opt` clone parameters internally
- For bulk operations, consider `query_raw` or `execute_raw`

## TLS Configuration

For production use, configure TLS:

```rust
// Without TLS (development only)
use tokio_postgres::NoTls;
let config = PGRobustClientConfig::new(url, NoTls);

// With rustls
// Add: tokio-postgres-rustls = "0.12"
use tokio_postgres_rustls::MakeRustlsConnect;
let tls = MakeRustlsConnect::new(rustls_config);
let config = PGRobustClientConfig::new(url, tls);

// With native-tls
// Add: postgres-native-tls = "0.5"
use postgres_native_tls::MakeTlsConnector;
let tls = MakeTlsConnector::new(native_tls_connector);
let config = PGRobustClientConfig::new(url, tls);
```

## Requirements

- Rust 1.85+ (Edition 2024)
- Tokio runtime with `rt` and `time` features
- PostgreSQL 9.0+ (LISTEN/NOTIFY support)

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed history of changes.

## License

MIT License - see [LICENSE](LICENSE) for details.
