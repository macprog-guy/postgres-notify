![Maintenance](https://img.shields.io/badge/maintenance-activly--developed-brightgreen.svg)
[![Rust](https://github.com/macprog-guy/postgres-notify/actions/workflows/tests.yml/badge.svg)](https://github.com/macprog-guy/postgres-notify/actions/workflows/tests.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# postgres-notify


[`PGNotifier`] makes it easy to subscribe to PostgreSQL notifications.
There are few examples in Rust that show how to capture these notifications
mostly because tokio_postgres examples spawn off the connection half such
that you can't listen for notifications anymore. [`PGNotifier`] also spawns
a task for the connection, but it also listens for notifications.

[`PGNotifier`] maintains a two list of callback functions, which are called
every time the it receives a notification. These two lists match the types
of notifications sent by Postgres: `NOTIFY` and `RAISE`.

[`PGRobustNotifier`] takes it a step further by wrapping a [`PGNotifier`] 
and providing automatic reconnection when a closed connection is detected.
It currently performs an infinite loop until such time the connection is 
established using exponential backoff with jitter but with a maximum 
backoff time of 1 minute + jitter. 


## LISTEN/NOTIFY

For a very long time (at least since version 7.1) postgres has supported
asynchronous notifications based on LISTEN/NOTIFY commands. This allows
the database to send notifications to the client in an "out-of-band"
channel.

Once the client has issued a `LISTEN <channel>` command, the database will
send notifications to the client whenever a `NOTIFY <channel> <payload>`
is issued on the database regardless of which session has issued it.
This can act as a cheap alternative to a pubsub system.

When calling `subscribe_notify` with a channel name, [`PGNotifier`] will
call the supplied closure upon receiving a NOTIFY message but only if it
matches the requested channel name.

```rust
use postgres_notify::PGNotifier;

let mut notifier = PGNotifier::spawn(client, conn);

notifier.subscribe_notify("test-channel", |notify| {
    println!("[{}]: {}", &notify.channel, &notify.payload);
});
```


## RAISE/LOGS

Logs in PostgreSQL are created by issuing `RAISE <level> <message>` commands
within your functions, stored procedures and scripts. When such a command is
issued, [`PGNotify`] receives a notification even if the call is in progress,
which allows a user to capture the execution log in realtime.

[`PGNotify`] simplifies log collection in two ways: first it provides the
`subscribe_raise` function, which registers a callback. Second, it also
provides the [`capture_log`](PGNotifier::capture_log) and
[`with_captured_log`](PGNotifier::with_captured_log) functions.

```rust
use postgres_notify::PGNotifier;

let mut notifier = PGNotifier::spawn(client, conn);

notifier.subscribe_raise(|notice| {
    // Will print the below message to stdout
    println!("{}", &notice);
});

// Will capture the notices in a Vec
let (_, log) = notifier.with_captured_log(async |client| {
    client.batch_execute(r#"
       do $$
       begin
           raise debug 'this is a DEBUG notification';
           raise log 'this is a LOG notification';
           raise info 'this is a INFO notification';
           raise notice 'this is a NOTICE notification';
           raise warning 'this is a WARNING notification';
       end;
       $$
    "#).await;
    Ok(())
}).await?

println!("{:#?}", &log);
```

You can look at the unit tests for a more in-depth example.

