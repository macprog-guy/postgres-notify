use {
    crate::{PGMessage, PGResult, PGRobustClientConfig, notify},
    std::{
        ops::{Deref, DerefMut},
        sync::{Arc, RwLock},
    },
    tokio::task::JoinHandle,
    tokio_postgres::{Socket, tls::MakeTlsConnect},
};

pub struct PGClient {
    pub(crate) client: tokio_postgres::Client,
    pub(crate) conn_handle: JoinHandle<()>,
    pub(crate) cancel_token: tokio_postgres::CancelToken,
    pub(crate) log: Arc<RwLock<Vec<PGMessage>>>,
}

impl PGClient {
    pub(crate) async fn connect<TLS: Clone>(config: &PGRobustClientConfig<TLS>) -> PGResult<Self>
    where
        TLS: MakeTlsConnect<Socket> + Clone,
        <TLS as MakeTlsConnect<Socket>>::Stream: Send + Sync + 'static,
    {
        //
        let (client, conn) =
            tokio_postgres::connect(&config.database_url, config.make_tls.clone()).await?;
        let cancel_token = client.cancel_token();
        let log = Arc::new(RwLock::new(Vec::default()));

        let conn_handle = {
            let log = log.clone();
            let callback = config.callback.clone();
            tokio::spawn(notify::handle_connection_polling(
                conn,
                move |msg: PGMessage| {
                    callback(msg.clone());
                    if let Ok(mut log) = log.write() {
                        log.push(msg);
                    }
                },
            ))
        };

        Ok(Self {
            client,
            conn_handle,
            cancel_token,
            log,
        })
    }

    pub async fn issue_listen(
        &self,
        channels: &[impl AsRef<str> + Send + Sync + 'static],
    ) -> Result<(), tokio_postgres::Error> {
        let channels: Vec<&str> = channels.into_iter().map(AsRef::as_ref).collect();

        // Build a sequence of `LISTEN` commands
        let sql =
            channels
                .iter()
                .fold(String::with_capacity(channels.len() * 32), |mut sql, ch| {
                    sql.push_str("LISTEN ");
                    sql.push_str(ch);
                    sql.push_str(";\n");
                    sql
                });

        // Tell the world we are about to subscribe
        #[cfg(feature = "tracing")]
        tracing::info!("Subscribing to channels: \"{}\"", &channels.join(","));

        // Issue the `LISTEN` commands
        self.simple_query(&sql).await?;
        Ok(())
    }

    pub async fn issue_unlisten(
        &self,
        channels: &[impl AsRef<str> + Send + Sync + 'static],
    ) -> Result<(), tokio_postgres::Error> {
        let channels: Vec<&str> = channels.into_iter().map(AsRef::as_ref).collect();

        // Build a sequence of `UNLISTEN` commands
        let sql =
            channels
                .iter()
                .fold(String::with_capacity(channels.len() * 32), |mut sql, ch| {
                    sql.push_str("UNLISTEN ");
                    sql.push_str(ch);
                    sql.push_str(";\n");
                    sql
                });

        // Tell the world we are about to subscribe
        #[cfg(feature = "tracing")]
        tracing::info!("Unsubscribing from channels: \"{}\"", &channels.join(","));

        // Issue the `UNLISTEN` commands
        self.simple_query(&sql).await?;
        Ok(())
    }

    pub async fn issue_unlisten_all(client: &PGClient) -> PGResult<()> {
        client.simple_query("UNLISTEN *").await?;
        Ok(())
    }
}

impl Drop for PGClient {
    fn drop(&mut self) {
        self.conn_handle.abort();
    }
}

impl Deref for PGClient {
    type Target = tokio_postgres::Client;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

impl DerefMut for PGClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.client
    }
}
