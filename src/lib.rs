//!
//! `postgres-notify` started out as an easy way to receive PostgreSQL
//! notifications but has since evolved into a much more useful client
//! and is able to handle the following:
//!
//! - Receive `NOTIFY <channel> <payload>` pub/sub style notifications
//!
//! - Receive `RAISE` messages and collects execution logs
//!
//! - Applies a timeout to all queries. If a query timesout then the
//!   client will attempt to cancel the ongoing query before returning
//!   an error.
//!
//! - Supports cancelling an ongoing query.
//!
//! - Automatically reconnects if the connection is lost and uses
//!   exponential backoff with jitter to avoid thundering herd effect.
//!
//! - Has a familiar API with an additional `timeout` argument.
//!
//!
//!
//! # BREAKING CHANGE in v0.3.0
//!
//! This latest version is a breaking change. The `PGNotifyingClient` has
//! been renamed `PGRobustClient` and queries don't need to be made through
//! the inner client anymore. Furthermore, a single callback handles all
//! of the notifications: NOTIFY, RAISE, TIMOUT, RECONNECT.
//!
//!
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
//! This can act as a cheap alternative to a pub/sub system though without
//! mailboxes or persistence.
//!
//! When calling `subscribe_notify` with a list of channel names, [`PGRobustClient`]
//! will the client callback any time a `NOTIFY` message is received for any of
//! the subscribed channels.
//!
//! ```rust
//! use postgres_notify::{PGRobustClient, PGMessage};
//! use tokio_postgres::NoTls;
//! use std::time::Duration;
//!
//! let rt = tokio::runtime::Builder::new_current_thread()
//!     .enable_io()
//!     .enable_time()
//!     .build()
//!     .expect("could not start tokio runtime");
//!
//! rt.block_on(async move {
//!     let database_url = "postgres://postgres:postgres@localhost:5432/postgres";
//!     let callback = |msg:PGMessage| println!("{:?}", &msg);
//!     let mut client = PGRobustClient::spawn(database_url, NoTls, callback)
//!         .await.expect("Could not connect to postgres");
//!
//!     client.subscribe_notify(&["test"], Some(Duration::from_millis(100)))
//!         .await.expect("Could not subscribe to channels");
//! });
//! ```
//!
//!
//!
//! # RAISE/LOGS
//!
//! Logs in PostgreSQL are created by writing `RAISE <level> <message>` statements
//! within your functions, stored procedures and scripts. When such a command is
//! issued, [`PGRobustClient`] receives a notification even if the call is still
//! in progress. This allows the caller to capture the execution log in realtime
//! if needed.
//!
//! [`PGRobustClient`] simplifies log collection in two ways. Firstly it provides
//! the [`with_captured_log`](PGRobustClient::with_captured_log) functions,
//! which collects the execution log and returns it along with the query result.
//! This is probably what most people will want to use.
//!
//! If your needs are more complex or if you want to propagate realtime logs,
//! then using client callback can be used to forwand the message on an
//! asynchonous channel.
//!
//! ```rust
//! use postgres_notify::{PGRobustClient, PGMessage};
//! use tokio_postgres::NoTls;
//! use std::time::Duration;
//!
//! let rt = tokio::runtime::Builder::new_current_thread()
//!     .enable_io()
//!     .enable_time()
//!     .build()
//!     .expect("could not start tokio runtime");
//!
//! rt.block_on(async move {
//!
//!     let callback = |msg:PGMessage| println!("{:?}", &msg);
//!
//!     let database_url = "postgres://postgres:postgres@localhost:5432/postgres";
//!     let mut client = PGRobustClient::spawn(database_url, NoTls, callback)
//!         .await.expect("Could not connect to postgres");
//!
//!     // Will capture the notices in a Vec
//!     let (_, log) = client.with_captured_log(async |client| {
//!         client.simple_query("
//!             do $$
//!             begin
//!                 raise debug 'this is a DEBUG notification';
//!                 raise log 'this is a LOG notification';
//!                 raise info 'this is a INFO notification';
//!                 raise notice 'this is a NOTICE notification';
//!                 raise warning 'this is a WARNING notification';
//!             end;
//!             $$",
//!             Some(Duration::from_secs(1))
//!         ).await.expect("Error during query execution");
//!         Ok(())
//!     }).await.expect("Error during captur log");
//!
//!     println!("{:#?}", &log);
//!  });
//! ```
//!
//! Note that the client passed to the async callback is `&mut self`, which
//! means that all queries within that block are subject to the same timeout
//! and reconnect handling.
//!
//! You can look at the unit tests for a more in-depth example.
//!
//!
//!
//! # TIMEOUT
//!
//! All of the query functions in [`PGRobustClient`] have a `timeout` argument.
//! If the query takes longer than the timeout, then an error is returned.
//! If not specified, the default timeout is 1 hour.
//!
//!
//! # RECONNECT
//!
//! If the connection to the database is lost, then [`PGRobustClient`] will
//! attempt to reconnect to the database automatically. If the maximum number
//! of reconnect attempts is reached then an error is returned. Furthermore,
//! it uses a exponential backoff with jitter in order to avoid thundering
//! herd effect.
//!

mod error;
mod messages;
mod notify;

pub use error::*;
pub use messages::*;
use tokio_postgres::{SimpleQueryMessage, ToStatement};

use {
    futures::TryFutureExt,
    std::{
        collections::BTreeSet,
        sync::{Arc, RwLock},
        time::Duration,
    },
    tokio::{
        task::JoinHandle,
        time::{sleep, timeout},
    },
    tokio_postgres::{
        CancelToken, Client as PGClient, Row, RowStream, Socket, Statement, Transaction,
        tls::MakeTlsConnect,
        types::{BorrowToSql, ToSql, Type},
    },
};

/// Shorthand for Result with tokio_postgres::Error
pub type PGResult<T> = Result<T, PGError>;

pub struct PGRobustClient<TLS>
where
    TLS: MakeTlsConnect<Socket>,
{
    database_url: String,
    make_tls: TLS,
    client: PGClient,
    conn_handle: JoinHandle<()>,
    cancel_token: CancelToken,
    subscriptions: BTreeSet<String>,
    callback: Arc<dyn Fn(PGMessage) + Send + Sync + 'static>,
    max_reconnect_attempts: u32,
    default_timeout: Duration,
    log: Arc<RwLock<Vec<PGMessage>>>,
}

#[allow(unused)]
impl<TLS> PGRobustClient<TLS>
where
    TLS: MakeTlsConnect<Socket> + Clone,
    <TLS as MakeTlsConnect<Socket>>::Stream: Send + Sync + 'static,
{
    ///
    /// Given a connect factory and a callback, returns a new [`PGRobustClient`].
    ///
    /// The callback will be called whenever a new NOTIFY/RAISE message is received.
    /// Furthermore, it is also called with a [`PGMessage::Timeout`], when a query
    /// times out, [`PGMessage::Disconnected`] if the internal state of the client
    /// is not as expected (Poisoned lock, dropped connections, etc.) or
    /// [`PGMessage::Reconnect`] whenever a new reconnect attempt is made.
    ///
    pub async fn spawn(
        database_url: impl AsRef<str>,
        make_tls: TLS,
        callback: impl Fn(PGMessage) + Send + Sync + 'static,
    ) -> PGResult<Self> {
        //
        // Setup log and other default values
        //
        let log = Arc::new(RwLock::new(Vec::default()));
        let default_timeout = Duration::from_secs(60 * 60);

        //
        // We wrap the callback so that it also inserts into the log.
        //
        // NOTE: we need to type erase here because otherwise the call to Self::connect
        //      will not compile.
        //
        let callback: Arc<dyn Fn(PGMessage) + Send + Sync + 'static> = Arc::new({
            let log = log.clone();
            move |msg: PGMessage| {
                callback(msg.clone());
                if let Ok(mut log) = log.write() {
                    log.push(msg);
                }
            }
        });

        // Connect to the database
        let (client, conn_handle, cancel_token) =
            Self::connect(database_url.as_ref(), &make_tls, &callback).await?;

        Ok(Self {
            database_url: database_url.as_ref().to_string(),
            make_tls,
            client,
            conn_handle,
            cancel_token,
            subscriptions: BTreeSet::new(),
            callback,
            max_reconnect_attempts: u32::MAX,
            default_timeout,
            log,
        })
    }

    ///
    /// Sets the default timeout for all queries. Defaults to 1 hour.
    ///
    /// This function consumes and returns self and is therefor usually used
    /// just after [`PGRobustClient::spawn`].
    ///
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    ///
    /// Sets the maximum number of reconnect attempts before giving up.
    /// Defaults to `u32::MAX`.
    ///
    /// This function consumes and returns self and is therefor usually used
    /// just after [`PGRobustClient::spawn`].
    ///
    pub fn with_max_reconnect_attempts(mut self, max_attempts: u32) -> Self {
        self.max_reconnect_attempts = max_attempts;
        self
    }

    ///
    /// PRIVATE
    /// Does the necessary details to connect to the database and hookup callbacks and notifications.
    ///
    async fn connect(
        database_url: &str,
        make_tls: &TLS,
        callback: &Arc<dyn Fn(PGMessage) + Send + Sync + 'static>,
    ) -> PGResult<(PGClient, JoinHandle<()>, CancelToken)> {
        //
        let (client, conn) = tokio_postgres::connect(database_url, make_tls.clone()).await?;
        let cancel_token = client.cancel_token();

        let callback = callback.clone();
        let handle = tokio::spawn(notify::handle_connection_polling(conn, move |msg| {
            callback(msg)
        }));

        Ok((client, handle, cancel_token))
    }

    ///
    /// Cancels any in-progress query.
    ///
    /// This is the only function that does not take a timeout nor does it
    /// attempt to reconnect if the connection is lost. It will simply
    /// return the original error.
    ///
    pub async fn cancel_query(&mut self) -> PGResult<()> {
        self.cancel_token
            .cancel_query(self.make_tls.clone())
            .await
            .map_err(Into::into)
    }

    ///
    /// Returns the log messages captured since the last call to this function.
    /// It also clears the log.
    ///
    pub fn capture_and_clear_log(&mut self) -> Vec<PGMessage> {
        if let Ok(mut guard) = self.log.write() {
            let empty_log = Vec::default();
            std::mem::replace(&mut *guard, empty_log)
        } else {
            Vec::default()
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
    pub async fn with_captured_log<F, T>(&mut self, f: F) -> PGResult<(T, Vec<PGMessage>)>
    where
        F: AsyncFn(&mut Self) -> PGResult<T>,
    {
        self.capture_and_clear_log(); // clear the log just in case...
        let result = f(self).await?;
        let log = self.capture_and_clear_log();
        Ok((result, log))
    }

    ///
    /// Attempts to reconnect after a connection loss.
    ///
    /// Reconnection applies an exponention backoff with jitter in order to
    /// avoid thundering herd effect. If the maximum number of attempts is
    /// reached then an error is returned.
    ///
    /// If an error unrelated to establishing a new connection is returned
    /// when trying to connect then that error is returned.
    ///
    pub async fn reconnect(&mut self) -> PGResult<()> {
        //
        use std::cmp::{max, min};
        let mut attempts = 1;
        let mut k = 500;

        while attempts <= self.max_reconnect_attempts {
            //
            // Implement exponential backoff + jitter
            // Initial delay will be 500ms, max delay is 1h.
            //
            sleep(Duration::from_millis(k + rand::random_range(0..k / 2))).await;
            k = min(k * 2, 60000);

            tracing::info!("Reconnect attempt #{}", attempts);
            (self.callback)(PGMessage::reconnect(attempts, self.max_reconnect_attempts));

            attempts += 1;

            let maybe_triple =
                Self::connect(&self.database_url, &self.make_tls, &self.callback).await;

            match maybe_triple {
                Ok((client, conn_handle, cancel_token)) => {
                    // Abort the old connection just in case
                    self.conn_handle.abort();

                    self.client = client;
                    self.conn_handle = conn_handle;
                    self.cancel_token = cancel_token;

                    // Resubscribe to previously subscribed channels
                    let subs: Vec<_> = self.subscriptions.iter().map(String::from).collect();

                    match Self::subscribe_notify_impl(&self.client, &subs).await {
                        Ok(_) => {
                            return Ok(());
                        }
                        Err(e) if is_pg_connection_issue(&e) => {
                            continue;
                        }
                        Err(e) => {
                            return Err(e.into());
                        }
                    }
                }
                Err(e) if e.is_pg_connection_issue() => {
                    continue;
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        // Issue the failed to reconnect message
        (self.callback)(PGMessage::failed_to_reconnect(self.max_reconnect_attempts));
        // Return the error
        Err(PGError::FailedToReconnect(self.max_reconnect_attempts))
    }

    pub async fn wrap_reconnect<T>(
        &mut self,
        max_dur: Option<Duration>,
        factory: impl AsyncFn(&mut PGClient) -> Result<T, tokio_postgres::Error>,
    ) -> PGResult<T> {
        let max_dur = max_dur.unwrap_or(self.default_timeout);
        loop {
            match timeout(max_dur, factory(&mut self.client)).await {
                // Query succeeded so return the result
                Ok(Ok(o)) => return Ok(o),
                // Query failed because of connection issues
                Ok(Err(e)) if is_pg_connection_issue(&e) => {
                    self.reconnect().await?;
                }
                // Query failed for some other reason
                Ok(Err(e)) => {
                    return Err(e.into());
                }
                // Query timed out!
                Err(_) => {
                    // Callback with timeout message
                    (self.callback)(PGMessage::timeout(max_dur));
                    // Cancel the ongoing query
                    let status = self.cancel_token.cancel_query(self.make_tls.clone()).await;
                    // Callback with cancelled message
                    (self.callback)(PGMessage::cancelled(!status.is_err()));
                    // Return the timeout error
                    return Err(PGError::Timeout(max_dur));
                }
            }
        }
    }

    pub async fn subscribe_notify(
        &mut self,
        channels: &[impl AsRef<str> + Send + Sync + 'static],
        timeout: Option<Duration>,
    ) -> PGResult<()> {
        if !channels.is_empty() {
            self.wrap_reconnect(timeout, async |client: &mut PGClient| {
                Self::subscribe_notify_impl(client, channels).await
            })
            .await?;

            // Add to our subscriptions
            channels.iter().for_each(|ch| {
                self.subscriptions.insert(ch.as_ref().to_string());
            });
        }
        Ok(())
    }

    async fn subscribe_notify_impl(
        client: &PGClient,
        channels: &[impl AsRef<str> + Send + Sync + 'static],
    ) -> Result<(), tokio_postgres::Error> {
        // Build a sequence of `LISTEN` commands
        let sql = channels
            .iter()
            .map(|ch| format!("LISTEN {};", ch.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");

        // Tell the world we are about to subscribe
        #[cfg(feature = "tracing")]
        tracing::info!(
            "Subscribing to channels: \"{}\"",
            &channels
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>()
                .join(",")
        );

        // Issue the `LISTEN` commands
        client.simple_query(&sql).await?;
        Ok(())
    }

    pub async fn unsubscribe_notify(
        &mut self,
        channels: &[impl AsRef<str> + Send + Sync + 'static],
        timeout: Option<Duration>,
    ) -> PGResult<()> {
        if !channels.is_empty() {
            self.wrap_reconnect(timeout, async move |client: &mut PGClient| {
                // Build a sequence of `LISTEN` commands
                let sql = channels
                    .iter()
                    .map(|ch| format!("UNLISTEN {};", ch.as_ref()))
                    .collect::<Vec<_>>()
                    .join("\n");

                // Tell the world we are about to subscribe
                #[cfg(feature = "tracing")]
                tracing::info!(
                    "Unsubscribing from channels: \"{}\"",
                    &channels
                        .iter()
                        .map(AsRef::as_ref)
                        .collect::<Vec<_>>()
                        .join(",")
                );

                // Issue the `LISTEN` commands
                client.simple_query(&sql).await?;
                Ok(())
            })
            .await?;

            // Remove subscriptions
            channels.iter().for_each(|ch| {
                self.subscriptions.remove(ch.as_ref());
            });
        }
        Ok(())
    }

    ///
    /// Unsubscribes from all channels.
    ///
    pub async fn unsubscribe_notify_all(&mut self, timeout: Option<Duration>) -> PGResult<()> {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            // Tell the world we are about to unsubscribe
            #[cfg(feature = "tracing")]
            tracing::info!("Unsubscribing from channels: *");
            // Issue the `UNLISTEN` commands
            client.simple_query("UNLISTEN *").await?;
            Ok(())
        })
        .await
    }

    /// Like [`Client::execute_raw`].
    pub async fn execute_raw<P, I, T>(
        &mut self,
        statement: &T,
        params: I,
        timeout: Option<Duration>,
    ) -> PGResult<u64>
    where
        T: ?Sized + ToStatement + Sync + Send,
        P: BorrowToSql + Clone + Send + Sync,
        I: IntoIterator<Item = P> + Sync + Send,
        I::IntoIter: ExactSizeIterator,
    {
        let params: Vec<_> = params.into_iter().collect();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.execute_raw(statement, params.clone()).await
        })
        .await
    }

    /// Like [`Client::query`].
    pub async fn query<T>(
        &mut self,
        query: &T,
        params: &[&(dyn ToSql + Sync)],
        timeout: Option<Duration>,
    ) -> PGResult<Vec<Row>>
    where
        T: ?Sized + ToStatement + Sync + Send,
    {
        let params = params.to_vec();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query(query, &params).await
        })
        .await
    }

    /// Like [`Client::query_one`].
    pub async fn query_one<T>(
        &mut self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
        timeout: Option<Duration>,
    ) -> PGResult<Row>
    where
        T: ?Sized + ToStatement + Sync + Send,
    {
        let params = params.to_vec();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query_one(statement, &params).await
        })
        .await
    }

    /// Like [`Client::query_opt`].
    pub async fn query_opt<T>(
        &mut self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
        timeout: Option<Duration>,
    ) -> PGResult<Option<Row>>
    where
        T: ?Sized + ToStatement + Sync + Send,
    {
        let params = params.to_vec();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query_opt(statement, &params).await
        })
        .await
    }

    /// Like [`Client::query_raw`].
    pub async fn query_raw<T, P, I>(
        &mut self,
        statement: &T,
        params: I,
        timeout: Option<Duration>,
    ) -> PGResult<RowStream>
    where
        T: ?Sized + ToStatement + Sync + Send,
        P: BorrowToSql + Clone + Send + Sync,
        I: IntoIterator<Item = P> + Sync + Send,
        I::IntoIter: ExactSizeIterator,
    {
        let params: Vec<_> = params.into_iter().collect();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query_raw(statement, params.clone()).await
        })
        .await
    }

    /// Like [`Client::query_typed`]
    pub async fn query_typed(
        &mut self,
        statement: &str,
        params: &[(&(dyn ToSql + Sync), Type)],
        timeout: Option<Duration>,
    ) -> PGResult<Vec<Row>> {
        let params = params.to_vec();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query_typed(statement, &params).await
        })
        .await
    }

    /// Like [`Client::query_typed_raw`]
    pub async fn query_typed_raw<P, I>(
        &mut self,
        statement: &str,
        params: I,
        timeout: Option<Duration>,
    ) -> PGResult<RowStream>
    where
        P: BorrowToSql + Clone + Send + Sync,
        I: IntoIterator<Item = (P, Type)> + Sync + Send,
    {
        let params: Vec<_> = params.into_iter().collect();
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.query_typed_raw(statement, params.clone()).await
        })
        .await
    }

    /// Like [`Client::prepare`].
    pub async fn prepare(&mut self, query: &str, timeout: Option<Duration>) -> PGResult<Statement> {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.prepare(query).map_err(Into::into).await
        })
        .await
    }

    /// Like [`Client::prepare_typed`].
    pub async fn prepare_typed(
        &mut self,
        query: &str,
        parameter_types: &[Type],
        timeout: Option<Duration>,
    ) -> PGResult<Statement> {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.prepare_typed(query, parameter_types).await
        })
        .await
    }

    //
    /// Similar but not quite the same as [`Client::transaction`].
    ///
    /// Executes the closure as a single transaction.
    /// Commit is automatically called after the closure. If any connection
    /// issues occur during the transaction then the transaction is rolled
    /// back (on drop) and retried a new with the new connection subject to
    /// the maximum number of reconnect attempts.
    ///
    pub async fn transaction<F>(&mut self, timeout: Option<Duration>, f: F) -> PGResult<()>
    where
        for<'a> F: AsyncFn(&'a mut Transaction) -> Result<(), tokio_postgres::Error>,
    {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            let mut tx = client.transaction().await?;
            f(&mut tx).await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    /// Like [`Client::batch_execute`].
    pub async fn batch_execute(&mut self, query: &str, timeout: Option<Duration>) -> PGResult<()> {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.batch_execute(query).await
        })
        .await
    }

    /// Like [`Client::simple_query`].
    pub async fn simple_query(
        &mut self,
        query: &str,
        timeout: Option<Duration>,
    ) -> PGResult<Vec<SimpleQueryMessage>> {
        self.wrap_reconnect(timeout, async |client: &mut PGClient| {
            client.simple_query(query).await
        })
        .await
    }

    /// Returns a reference to the underlying [`Client`].
    pub fn client(&self) -> &PGClient {
        &self.client
    }
}

///
/// Wraps any future in a tokio timeout and maps the Elapsed error to a PGError::Timeout.
///
pub async fn wrap_timeout<T>(dur: Duration, fut: impl Future<Output = PGResult<T>>) -> PGResult<T> {
    match timeout(dur, fut).await {
        Ok(out) => out,
        Err(_) => Err(PGError::Timeout(dur)),
    }
}

#[cfg(test)]
mod tests {

    use {
        super::{PGError, PGMessage, PGRaiseLevel, PGRobustClient},
        insta::*,
        std::{
            sync::{Arc, RwLock},
            time::Duration,
        },
        testcontainers::{ImageExt, runners::AsyncRunner},
        testcontainers_modules::postgres::Postgres,
    };

    fn sql_for_log_and_notify_test(level: PGRaiseLevel) -> String {
        format!(
            r#"
                    set client_min_messages to '{}';
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
            level
        )
    }

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

        // NOTE: this stuff with Box::leak allows us to create a static string
        let database_url = format!(
            "postgres://postgres:postgres@{}:{}/postgres",
            pg_server.get_host().await.unwrap(),
            pg_server.get_host_port_ipv4(5432).await.unwrap()
        );

        // let database_url = "postgres://postgres:postgres@localhost:5432/postgres";

        // --------------------------------------------------------------------
        // Connect to the server
        // --------------------------------------------------------------------

        let notices = Arc::new(RwLock::new(Vec::new()));
        let notices_clone = notices.clone();

        let callback = move |msg: PGMessage| {
            if let Ok(mut guard) = notices_clone.write() {
                guard.push(msg.to_string());
            }
        };

        let mut admin = PGRobustClient::spawn(&database_url, tokio_postgres::NoTls, |_| {})
            .await
            .expect("could not create initial client");

        let mut client = PGRobustClient::spawn(&database_url, tokio_postgres::NoTls, callback)
            .await
            .expect("could not create initial client")
            .with_max_reconnect_attempts(2);

        // --------------------------------------------------------------------
        // Subscribe to notify and raise
        // --------------------------------------------------------------------

        client
            .subscribe_notify(&["test"], None)
            .await
            .expect("could not subscribe");

        let (_, execution_log) = client
            .with_captured_log(async |client: &mut PGRobustClient<_>| {
                client
                    .simple_query(&sql_for_log_and_notify_test(PGRaiseLevel::Debug), None)
                    .await
            })
            .await
            .expect("could not execute queries on postgres");

        assert_json_snapshot!("subscribed-executionlog", &execution_log, {
            "[].timestamp" => "<timestamp>",
            "[].process_id" => "<pid>",
        });

        assert_snapshot!("subscribed-notify", extract_and_clear_logs(&notices));

        // --------------------------------------------------------------------
        // Unsubscribe
        // --------------------------------------------------------------------

        client
            .unsubscribe_notify(&["test"], None)
            .await
            .expect("could not unsubscribe");

        let (_, execution_log) = client
            .with_captured_log(async |client| {
                client
                    .simple_query(&sql_for_log_and_notify_test(PGRaiseLevel::Warning), None)
                    .await
            })
            .await
            .expect("could not execute queries on postgres");

        assert_json_snapshot!("unsubscribed-executionlog", &execution_log, {
            "[].timestamp" => "<timestamp>",
            "[].process_id" => "<pid>",
        });

        assert_snapshot!("unsubscribed-notify", extract_and_clear_logs(&notices));

        // --------------------------------------------------------------------
        // Timeout
        // --------------------------------------------------------------------

        let result = client
            .simple_query(
                "
                    do $$
                    begin
                        raise info 'before sleep';
                        perform pg_sleep(3);
                        raise info 'after sleep';
                    end;
                    $$
                ",
                Some(Duration::from_secs(1)),
            )
            .await;

        assert!(matches!(result, Err(PGError::Timeout(_))));
        assert_snapshot!("timeout-messages", extract_and_clear_logs(&notices));

        // --------------------------------------------------------------------
        // Reconnect (before query)
        // --------------------------------------------------------------------

        admin.simple_query("select pg_terminate_backend(pid) from pg_stat_activity where pid != pg_backend_pid()", None)
            .await.expect("could not kill other client");

        let result = client
            .simple_query(
                "
                    do $$
                    begin
                        raise info 'before sleep';
                        perform pg_sleep(1);
                        raise info 'after sleep';
                    end;
                    $$
                ",
                Some(Duration::from_secs(10)),
            )
            .await;

        assert!(matches!(result, Ok(_)));
        assert_snapshot!("reconnect-before", extract_and_clear_logs(&notices));

        // --------------------------------------------------------------------
        // Reconnect (during query)
        // --------------------------------------------------------------------

        let query = client.simple_query(
            "
                    do $$
                    begin
                        raise info 'before sleep';
                        perform pg_sleep(1);
                        raise info 'after sleep';
                    end;
                    $$
                ",
            None,
        );

        let kill_later = 
            admin.simple_query("
                select pg_sleep(0.5); 
                select pg_terminate_backend(pid) from pg_stat_activity where pid != pg_backend_pid()", 
                None
            );

        let (_, result) = tokio::join!(kill_later, query);

        assert!(matches!(result, Ok(_)));
        assert_snapshot!("reconnect-during", extract_and_clear_logs(&notices));

        // --------------------------------------------------------------------
        // Reconnect (failure)
        // --------------------------------------------------------------------

        pg_server.stop().await.expect("could not stop server");

        let result = client.simple_query(
            "
                do $$
                begin
                    raise info 'before sleep';
                    perform pg_sleep(1);
                    raise info 'after sleep';
                end;
                $$
            ",
            None,
        ).await;

        eprintln!("result: {result:?}");
        assert!(matches!(result, Err(PGError::FailedToReconnect(2))));
        assert_snapshot!("reconnect-failure", extract_and_clear_logs(&notices));


    }

    fn extract_and_clear_logs(logs: &Arc<RwLock<Vec<String>>>) -> String {
        let mut guard = logs.write().expect("could not read notices");
        let emtpy_log = Vec::default();
        let log = std::mem::replace(&mut *guard, emtpy_log);
        redact_pids(&redact_timestamps(&log.join("\n")))
    }

    fn redact_timestamps(text: &str) -> String {
        use regex::Regex;
        use std::sync::OnceLock;
        pub static TIMESTAMP_PATTERN: OnceLock<Regex> = OnceLock::new();
        let pat = TIMESTAMP_PATTERN.get_or_init(|| {
            Regex::new(r"\d{4}-\d{2}-\d{2}.?\d{2}:\d{2}:\d{2}(\.\d{3,9})?(Z| UTC|[+-]\d{2}:\d{2})?")
                .unwrap()
        });
        pat.replace_all(text, "<timestamp>").to_string()
    }

    fn redact_pids(text: &str) -> String {
        use regex::Regex;
        use std::sync::OnceLock;
        pub static TIMESTAMP_PATTERN: OnceLock<Regex> = OnceLock::new();
        let pat = TIMESTAMP_PATTERN.get_or_init(|| Regex::new(r"pid=\d+").unwrap());
        pat.replace_all(text, "<pid>").to_string()
    }
}
