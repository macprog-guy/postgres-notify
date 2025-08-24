![Maintenance](https://img.shields.io/badge/maintenance-activly--developed-brightgreen.svg)
[![Rust](https://github.com/macprog-guy/postgres-notify/actions/workflows/tests.yml/badge.svg)](https://github.com/macprog-guy/postgres-notify/actions/workflows/tests.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# postgres-notify


`postgres-notify` started out as an easy way to receive PostgreSQL
notifications but has since evolved into a much more useful client
and is able to handle the following:

- Receive `NOTIFY <channel> <payload>` pub/sub style notifications

- Receive `RAISE` messages and collects execution logs

- Applies a timeout to all queries. If a query timesout then the
  client will attempt to cancel the ongoing query before returning
  an error.

- Supports cancelling an ongoing query.

- Automatically reconnects if the connection is lost and uses
  exponential backoff with jitter to avoid thundering herd effect.

- Supports an `connect_script`, which can be executed on connect.

- Has a familiar API with an additional `timeout` argument.


## BREAKING CHANGE in v0.3.2

Configuration is done through the [`PGRobustClientConfig`] struct.


## BREAKING CHANGE in v0.3.0

This latest version is a breaking change. The `PGNotifyingClient` has
been renamed `PGRobustClient` and queries don't need to be made through
the inner client anymore. Furthermore, a single callback handles all
of the notifications: NOTIFY, RAISE, TIMOUT, RECONNECT.



## LISTEN/NOTIFY

For a very long time (at least since version 7.1) postgres has supported
asynchronous notifications based on LISTEN/NOTIFY commands. This allows
the database to send notifications to the client in an "out-of-band"
channel.

Once the client has issued a `LISTEN <channel>` command, the database will
send notifications to the client whenever a `NOTIFY <channel> <payload>`
is issued on the database regardless of which session has issued it.
This can act as a cheap alternative to a pub/sub system though without
mailboxes or persistence.

When calling `subscribe_notify` with a list of channel names, [`PGRobustClient`]
will the client callback any time a `NOTIFY` message is received for any of
the subscribed channels.

```rust
use postgres_notify::{PGRobustClientConfig, PGRobustClient, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

let rt = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .enable_time()
    .build()
    .expect("could not start tokio runtime");

rt.block_on(async move {

    let database_url = "postgres://postgres:postgres@localhost:5432/postgres";
    let config = PGRobustClientConfig::new(database_url, NoTls)
        .callback(|msg:PGMessage| println!("{:?}", &msg));

    let mut client = PGRobustClient::spawn(config)
        .await.expect("Could not connect to postgres");

    client.subscribe_notify(&["test"], Some(Duration::from_millis(100)))
        .await.expect("Could not subscribe to channels");
});
```



## RAISE/LOGS

Logs in PostgreSQL are created by writing `RAISE <level> <message>` statements
within your functions, stored procedures and scripts. When such a command is
issued, [`PGRobustClient`] receives a notification even if the call is still
in progress. This allows the caller to capture the execution log in realtime
if needed.

[`PGRobustClient`] simplifies log collection in two ways. Firstly it provides
the [`with_captured_log`](PGRobustClient::with_captured_log) functions,
which collects the execution log and returns it along with the query result.
This is probably what most people will want to use.

If your needs are more complex or if you want to propagate realtime logs,
then using client callback can be used to forwand the message on an
asynchonous channel.

```rust
use postgres_notify::{PGRobustClient, PGRobustClientConfig, PGMessage};
use tokio_postgres::NoTls;
use std::time::Duration;

let rt = tokio::runtime::Builder::new_current_thread()
    .enable_io()
    .enable_time()
    .build()
    .expect("could not start tokio runtime");

rt.block_on(async move {

    let database_url = "postgres://postgres:postgres@localhost:5432/postgres";
    let config = PGRobustClientConfig::new(database_url, NoTls)
        .callback(|msg:PGMessage| println!("{:?}", &msg));

    let mut client = PGRobustClient::spawn(config)
        .await.expect("Could not connect to postgres");

    // Will capture the notices in a Vec
    let (_, log) = client.with_captured_log(async |client| {
        client.simple_query("
            do $$
            begin
                raise debug 'this is a DEBUG notification';
                raise log 'this is a LOG notification';
                raise info 'this is a INFO notification';
                raise notice 'this is a NOTICE notification';
                raise warning 'this is a WARNING notification';
            end;
            $$",
            Some(Duration::from_secs(1))
        ).await.expect("Error during query execution");
        Ok(())
    }).await.expect("Error during captur log");

    println!("{:#?}", &log);
 });
```

Note that the client passed to the async callback is `&mut self`, which
means that all queries within that block are subject to the same timeout
and reconnect handling.

You can look at the unit tests for a more in-depth example.



## TIMEOUT

All of the query functions in [`PGRobustClient`] have a `timeout` argument.
If the query takes longer than the timeout, then an error is returned.
If not specified, the default timeout is 1 hour.


## RECONNECT

If the connection to the database is lost, then [`PGRobustClient`] will
attempt to reconnect to the database automatically. If the maximum number
of reconnect attempts is reached then an error is returned. Furthermore,
it uses a exponential backoff with jitter in order to avoid thundering
herd effect.


License: MIT
