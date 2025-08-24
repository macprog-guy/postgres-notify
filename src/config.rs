use {
    crate::PGMessage,
    std::{collections::BTreeSet, sync::Arc, time::Duration},
    tokio_postgres::{Socket, tls::MakeTlsConnect},
};

#[derive(Clone)]
pub struct PGRobustClientConfig<TLS> {
    pub(crate) database_url: String,
    pub(crate) make_tls: TLS,
    pub(crate) subscriptions: BTreeSet<String>,
    pub(crate) callback: Arc<dyn Fn(PGMessage) + Send + Sync + 'static>,
    pub(crate) max_reconnect_attempts: u32,
    pub(crate) default_timeout: Duration,
    pub(crate) connect_script: Option<String>,
    pub(crate) application_name: Option<String>,
}

impl<TLS> PGRobustClientConfig<TLS>
where
    TLS: MakeTlsConnect<Socket> + Clone,
    <TLS as MakeTlsConnect<Socket>>::Stream: Send + Sync + 'static,
{
    pub fn new(database_url: impl Into<String>, make_tls: TLS) -> PGRobustClientConfig<TLS> {
        PGRobustClientConfig {
            database_url: database_url.into(),
            make_tls,
            subscriptions: BTreeSet::new(),
            callback: Arc::new(|_| {}),
            max_reconnect_attempts: 10,
            default_timeout: Duration::from_secs(3600),
            connect_script: None,
            application_name: None,
        }
    }

    pub fn subscriptions(
        mut self,
        subscriptions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.subscriptions
            .extend(subscriptions.into_iter().map(Into::into));
        self
    }

    pub fn with_subscriptions(
        &mut self,
        subscriptions: impl IntoIterator<Item = impl Into<String>>,
    ) {
        self.subscriptions
            .extend(subscriptions.into_iter().map(Into::into));
    }

    pub fn without_subscriptions(
        &mut self,
        subscriptions: impl IntoIterator<Item = impl Into<String>>,
    ) {
        for s in subscriptions.into_iter().map(Into::into) {
            self.subscriptions.remove(&s);
        }
    }

    pub fn callback(mut self, callback: impl Fn(PGMessage) + Send + Sync + 'static) -> Self {
        self.callback = Arc::new(callback);
        self
    }

    pub fn with_callback(&mut self, callback: impl Fn(PGMessage) + Send + Sync + 'static) {
        self.callback = Arc::new(callback);
    }

    pub fn max_reconnect_attempts(mut self, max_reconnect_attempts: u32) -> Self {
        self.max_reconnect_attempts = max_reconnect_attempts;
        self
    }

    pub fn with_max_reconnect_attempts(&mut self, max_reconnect_attempts: u32) {
        self.max_reconnect_attempts = max_reconnect_attempts;
    }

    pub fn default_timeout(mut self, default_timeout: Duration) -> Self {
        self.default_timeout = default_timeout;
        self
    }

    pub fn with_default_timeout(&mut self, default_timeout: Duration) {
        self.default_timeout = default_timeout;
    }

    pub fn connect_script(mut self, connect_script: impl Into<String>) -> Self {
        self.connect_script = Some(connect_script.into());
        self
    }

    pub fn with_connect_script(&mut self, connect_script: impl Into<String>) {
        self.connect_script = Some(connect_script.into());
    }

    pub fn application_name(mut self, application_name: impl Into<String>) -> Self {
        self.application_name = Some(application_name.into());
        self
    }

    pub fn with_application_name(&mut self, application_name: impl Into<String>) {
        self.application_name = Some(application_name.into());
    }

    pub fn full_connect_script(&self) -> Option<String> {
        if self.application_name.is_none()
            && self.connect_script.is_none()
            && self.subscriptions.is_empty()
        {
            return None;
        }

        let mut script = String::with_capacity(512);
        if let Some(name) = self.application_name.as_ref() {
            script.push_str("SET application_name = '");
            script.push_str(name);
            script.push_str("';\n");
        }
        if let Some(sql) = self.connect_script.as_ref() {
            script.push_str(sql);
        }
        for sub in &self.subscriptions {
            script.push_str("LISTEN ");
            script.push_str(sub);
            script.push_str(";\n");
        }
        Some(script)
    }
}
