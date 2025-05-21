//!
//! [`PGNotifier`] makes it easy to subscribe to PostgreSQL notifications.
//!
//! There are few examples in Rust that show how to capture these notifications
//! mostly because tokio_postgres examples spawn off the connection half such
//! that you can't listen for notifications anymore. [`PGNotifier`] also spawns
//! a task for the connection, but it also listens for notifications.
//!
//! [`PGNotifier`] maintains a two list of callback functions, which are called
//! every time the it receives a notification. These two lists match the types
//! of notifications sent by Postgres: `NOTIFY` and `RAISE`.
//!
//! # LISTEN/NOTIFY
//!
//! For a very long time (at least since version 7.1) postgres has supported
//! asynchronous notifications based on LISTEN/NOTIFY commands. This allows
//! the database to send notifications to the client in an "out-of-band"
//! channel.
//!
//! Once the client has issued a `LISTEN <channel>` command, the database will
//! send notifications to the client whenever a `NOTIFY <channel> <payload>`
//! is issued on the database regardless of which session has issued it.
//! This can act as a cheap alternative to a pubsub system.
//!
//! When calling `subscribe_notify` with a channel name, [`PGNotifier`] will
//! call the supplied closure upon receiving a NOTIFY message but only if it
//! matches the requested channel name.
//!
//! ```rust
//! use postgres_notify::PGNotifier;
//!
//! let mut notifier = PGNotifier::spawn(client, conn);
//!
//! notifier.subscribe_notify("test-channel", |notify| {
//!     println!("[{}]: {}", &notify.channel, &notify.payload);
//! });
//! ```
//!
//!
//! # RAISE/LOGS
//!
//! Logs in PostgreSQL are created by issuing `RAISE <level> <message>` commands
//! within your functions, stored procedures and scripts. When such a command is
//! issued, [`PGNotify`] receives a notification even if the call is in progress,
//! which allows a user to capture the execution log in realtime.
//!
//! [`PGNotify`] simplifies log collection in two ways: first it provides the
//! `subscribe_raise` function, which registers a callback. Second, it also
//! provides the [`capture_log`](PGNotifier::capture_log) and
//! [`with_captured_log`](PGNotifier::with_captured_log) functions.
//!
//! ```rust
//! use postgres_notify::PGNotifier;
//!
//! let mut notifier = PGNotifier::spawn(client, conn);
//!
//! notifier.subscribe_raise(|notice| {
//!     // Will print the below message to stdout
//!     println!("{}", &notice);
//! });
//!
//! // Will capture the notices in a Vec
//! let (_, log) = notifier.with_captured_log(async |client| {
//!     client.batch_execute(r#"
//!        do $$
//!        begin
//!            raise debug 'this is a DEBUG notification';
//!            raise log 'this is a LOG notification';
//!            raise info 'this is a INFO notification';
//!            raise notice 'this is a NOTICE notification';
//!            raise warning 'this is a WARNING notification';
//!        end;
//!        $$
//!     "#).await;
//!     Ok(())
//! }).await?
//!
//! println!("{:#?}", &log);
//! ```
//!
//! You can look at the unit tests for a more in-depth example.
//!
#[cfg(feature = "chrono")]
use chrono::{DateTime, SecondsFormat, Utc};
#[cfg(not(feature = "chrono"))]
use std::time::SystemTime;

use {
    futures::{StreamExt, stream},
    std::{
        collections::BTreeMap,
        fmt::{self, Display},
        str::FromStr,
        sync::{Arc, RwLock},
    },
    tokio::{
        io::{AsyncRead, AsyncWrite},
        task::JoinHandle,
    },
    tokio_postgres::{
        AsyncMessage, Client as PGClient, Connection as PGConnection, Notification, error::DbError,
    },
};

/// Type used to store callbacks for LISTEN/NOTIFY calls.
pub type NotifyCallbacks =
    Arc<RwLock<BTreeMap<String, Vec<Box<dyn for<'a> Fn(&'a PGNotify) + Send + Sync + 'static>>>>>;

/// Type used to store callbacks for RAISE &lt;level&gt; &lt;message&gt; calls.
pub type RaiseCallbacks =
    Arc<RwLock<Vec<Box<dyn for<'a> Fn(&'a PGRaise) + Send + Sync + 'static>>>>;

///
/// Forwards PostgreSQL `NOTIFY` and `RAISE` commands to subscribers.
///
pub struct PGNotifier {
    pub client: PGClient,
    listen_handle: JoinHandle<()>,
    log: Arc<RwLock<Option<Vec<PGRaise>>>>,
    raise_callbacks: RaiseCallbacks,
    notify_callbacks: NotifyCallbacks,
}

impl PGNotifier {
    ///
    /// Spawns a new postgres client/connection pair.
    ///
    pub fn spawn<S, T>(client: PGClient, mut conn: PGConnection<S, T>) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let log = Arc::new(RwLock::new(Some(Vec::default())));
        let notify_callbacks: NotifyCallbacks = Arc::new(RwLock::new(BTreeMap::new()));
        let raise_callbacks: RaiseCallbacks = Arc::new(RwLock::new(Vec::new()));

        // Spawn the connection and poll for messages on it.
        let listen_handle = {
            //
            let log = log.clone();
            let notify_callbacks = notify_callbacks.clone();
            let raise_callbacks = raise_callbacks.clone();

            tokio::spawn(async move {
                //
                let mut stream =
                    stream::poll_fn(move |cx| conn.poll_message(cx).map_err(|e| panic!("{}", e)));

                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(AsyncMessage::Notice(raise)) => {
                            Self::handle_raise(&raise_callbacks, &log, raise)
                        }
                        Ok(AsyncMessage::Notification(notice)) => {
                            Self::handle_notify(&notify_callbacks, notice)
                        }
                        _ => {
                            #[cfg(feature = "tracing")]
                            tracing::error!("connection to the server was closed");
                            #[cfg(not(feature = "tracing"))]
                            eprintln!("connection to the server was closed");
                            break;
                        }
                    }
                }
            })
        };

        Self {
            client,
            listen_handle,
            log,
            notify_callbacks,
            raise_callbacks,
        }
    }

    ///
    /// Handles the notification of LISTEN/NOTIFY subscribers.
    ///
    fn handle_notify(callbacks: &NotifyCallbacks, note: Notification) {
        let notice = PGNotify::new(note.channel(), note.payload());
        if let Ok(guard) = callbacks.read() {
            if let Some(cbs) = guard.get(note.channel()) {
                for callback in cbs.iter() {
                    callback(&notice);
                }
            }
        }
    }

    ///
    /// Handles the notification of `RAISE <level> <message>` subscribers.
    ///
    fn handle_raise(
        callbacks: &RaiseCallbacks,
        log: &Arc<RwLock<Option<Vec<PGRaise>>>>,
        raise: DbError,
    ) {
        let log_item = PGRaise {
            #[cfg(feature = "chrono")]
            timestamp: Utc::now(),
            #[cfg(not(feature = "chrono"))]
            timestamp: SystemTime::now(),
            level: PGRaiseLevel::from_str(raise.severity()).unwrap_or(PGRaiseLevel::Error),
            message: raise.message().into(),
        };

        if let Ok(guard) = callbacks.read() {
            for callback in guard.iter() {
                callback(&log_item);
            }
        }

        if let Ok(mut guard) = log.write() {
            guard.as_mut().map(|log| log.push(log_item));
        }
    }

    ///
    /// Subscribes to notifications on a particular channel.
    ///
    /// The call will issue the `LISTEN` command to PostgreSQL. There is
    /// currently no mechanism to unsubscribe even though postgres does
    /// supports UNLISTEN.
    ///
    pub async fn subscribe_notify<F>(
        &mut self,
        channel: impl Into<String>,
        callback: F,
    ) -> Result<(), tokio_postgres::Error>
    where
        F: Fn(&PGNotify) + Send + Sync + 'static,
    {
        // Issue the listen command to postgres
        let channel = channel.into();
        self.client
            .execute(&format!("LISTEN {}", &channel), &[])
            .await?;

        // Add the callback to the list of callbacks
        if let Ok(mut guard) = self.notify_callbacks.write() {
            guard.entry(channel).or_default().push(Box::new(callback));
        }

        Ok(())
    }

    ///
    /// Subscribes to `RAISE <level> <message>` notifications.
    ///
    /// There is currently no mechanism to unsubscribe. This would only require
    /// returning some form of "token", which could be used to unsubscribe.
    ///
    pub fn subscribe_raise(&mut self, callback: impl Fn(&PGRaise) + Send + Sync + 'static) {
        if let Ok(mut guard) = self.raise_callbacks.write() {
            guard.push(Box::new(callback));
        }
    }

    ///
    /// Returns the accumulated log since the last capture.
    ///
    /// If the code being called issues many `RAISE` commands and you never
    /// call [`capture_log`](PGNotifier::capture_log), then eventually, you
    /// might run out of memory. To ensure that this does not happen, you
    /// might consider using [`with_captured_log`](PGNotifier::with_captured_log)
    /// instead.
    ///
    pub fn capture_log(&self) -> Option<Vec<PGRaise>> {
        if let Ok(mut guard) = self.log.write() {
            let captured = guard.take();
            *guard = Some(Vec::default());
            captured
        } else {
            None
        }
    }

    ///
    /// Given an async closure taking the postgres client, returns the result
    /// of said closure along with the accumulated log since the beginning of
    /// the closure.
    ///
    /// If you use query pipelining then collect the logs for all queries in
    /// the pipeline. Otherwise, the logs might not be what you expect.
    ///
    pub async fn with_captured_log<F, T>(
        &self,
        f: F,
    ) -> Result<(T, Vec<PGRaise>), tokio_postgres::Error>
    where
        F: AsyncFnOnce(&PGClient) -> Result<T, tokio_postgres::Error>,
    {
        self.capture_log(); // clear the log
        let result = f(&self.client).await?;
        let log = self.capture_log().unwrap_or_default();
        Ok((result, log))
    }
}

impl Drop for PGNotifier {
    fn drop(&mut self) {
        self.listen_handle.abort();
    }
}

///
/// Message received when a `NOTIFY [channel] [payload]` is issued on PostgreSQL.
///
#[derive(Debug, Clone)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
pub struct PGNotify {
    pub channel: String,
    pub payload: String,
}

impl PGNotify {
    pub fn new(channel: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            payload: payload.into(),
        }
    }
}

///
/// # Message received when a `raise <level> <message>` is issued on PostgreSQL.
///
#[derive(Debug, Clone)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
pub struct PGRaise {
    #[cfg(feature = "chrono")]
    pub timestamp: DateTime<Utc>,
    #[cfg(not(feature = "chrono"))]
    pub timestamp: SystemTime,
    pub level: PGRaiseLevel,
    pub message: String,
}

impl From<DbError> for PGRaise {
    #[cfg(feature = "chrono")]
    fn from(raise: DbError) -> Self {
        PGRaise {
            timestamp: Utc::now(),
            level: PGRaiseLevel::from_str(raise.severity()).unwrap_or(PGRaiseLevel::Error),
            message: raise.message().into(),
        }
    }

    #[cfg(not(feature = "chrono"))]
    fn from(raise: DbError) -> Self {
        PGRaise {
            timestamp: SystemTime::now(),
            level: PGRaiseLevel::from_str(raise.severity()).unwrap_or(PGRaiseLevel::Error),
            message: raise.message().into(),
        }
    }
}

impl Display for PGRaise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "chrono")]
        let ts = self.timestamp.to_rfc3339_opts(SecondsFormat::Millis, true);

        #[cfg(not(feature = "chrono"))]
        let ts = {
            let duration = self
                .timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap();
            let millis = duration.as_millis();
            format!("{}", millis)
        };

        write!(f, "{}{:>8}: {}", &ts, &self.level.as_ref(), self.message)
    }
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
#[cfg_attr(any(feature = "serde", test), serde(rename_all = "UPPERCASE"))]
pub enum PGRaiseLevel {
    Debug,
    Log,
    Info,
    Notice,
    Warning,
    Error,
    Fatal,
    Panic,
}

impl AsRef<str> for PGRaiseLevel {
    fn as_ref(&self) -> &str {
        use PGRaiseLevel::*;
        match self {
            Debug => "DEBUG",
            Log => "LOG",
            Info => "INFO",
            Notice => "NOTICE",
            Warning => "WARNING",
            Error => "ERROR",
            Fatal => "FATAL",
            Panic => "PANIC",
        }
    }
}

impl FromStr for PGRaiseLevel {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DEBUG" => Ok(PGRaiseLevel::Debug),
            "LOG" => Ok(PGRaiseLevel::Log),
            "INFO" => Ok(PGRaiseLevel::Info),
            "NOTICE" => Ok(PGRaiseLevel::Notice),
            "WARNING" => Ok(PGRaiseLevel::Warning),
            "ERROR" => Ok(PGRaiseLevel::Error),
            "FATAL" => Ok(PGRaiseLevel::Fatal),
            "PANIC" => Ok(PGRaiseLevel::Panic),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::{PGNotifier, PGNotify};
    use insta::*;
    use std::sync::{Arc, RwLock};
    use testcontainers::{ImageExt, runners::AsyncRunner};
    use testcontainers_modules::postgres::Postgres;

    #[tokio::test]
    async fn test_integration() {
        //
        // --------------------------------------------------------------------
        // Setup Postgres Server
        // --------------------------------------------------------------------

        let pg_server = Postgres::default()
            .with_tag("16.4")
            .start()
            .await
            .expect("could not start postgres server");

        let database_url = format!(
            "postgres://postgres:postgres@{}:{}/postgres",
            pg_server.get_host().await.unwrap(),
            pg_server.get_host_port_ipv4(5432).await.unwrap()
        );

        // --------------------------------------------------------------------
        // Connect to the server
        // --------------------------------------------------------------------

        let (client, conn) = tokio_postgres::connect(&database_url, tokio_postgres::NoTls)
            .await
            .expect("could not connect to postgres server");

        let mut notifier = PGNotifier::spawn(client, conn);

        // --------------------------------------------------------------------
        // Subscribe to notify and raise
        // --------------------------------------------------------------------

        let notices = Arc::new(RwLock::new(Vec::new()));
        let notices_clone = notices.clone();

        notifier
            .subscribe_notify("test", move |notify: &PGNotify| {
                if let Ok(mut guard) = notices_clone.write() {
                    guard.push(notify.clone());
                }
            })
            .await
            .expect("could not subscribe to notifications");

        let (_, execution_log) = notifier
            .with_captured_log(async |client| {
                client
                    .batch_execute(
                        r#"
                    set client_min_messages to 'debug';
                    do $$
                    begin
                        raise debug 'this is a DEBUG notification';
                        notify test, 'test#1';
                        raise log 'this is a LOG notification';
                        notify test, 'test#2';
                        raise info 'this is a INFO notification';
                        notify test, 'test#3';
                        raise notice 'this is a NOTICE notification';
                        notify test, 'test#4';
                        raise warning 'this is a WARNING notification';
                        notify test, 'test#5';
                    end;
                    $$;
                "#,
                    )
                    .await
            })
            .await
            .expect("could not execute queries on postgres");

        assert_json_snapshot!("raise-notices", &execution_log, {
            "[].timestamp" => "<timestamp>"
        });

        let guard = notices.read().expect("could not read notices");
        let raise_notices = guard.clone();
        assert_json_snapshot!("listen/notify", &raise_notices);
    }
}
