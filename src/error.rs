use {std::time::Duration, thiserror::Error};

#[derive(Debug, Error)]
pub enum PGError {
    #[error(transparent)]
    Postgres(tokio_postgres::Error),
    #[error("Query timed out after {0:?}")]
    Timeout(Duration),
    #[error("Failed to reconnect after {0} attempts")]
    FailedToReconnect(u32),
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl PGError {
    pub fn is_pg_connection_issue(&self) -> bool {
        matches!(self, PGError::Postgres(e) if is_pg_connection_issue(e))
    }
    pub fn is_timeout(&self) -> bool {
        matches!(self, PGError::Timeout(_))
    }

    pub fn other(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        PGError::Other(Box::new(err))
    }
}

impl From<tokio_postgres::Error> for PGError {
    fn from(e: tokio_postgres::Error) -> Self {
        PGError::Postgres(e)
    }
}

///
/// PRIVATE
/// Returns true if the error is a connection issue.
///
pub(crate) fn is_pg_connection_issue(err: &tokio_postgres::Error) -> bool {
    if err.is_closed() {
        return true;
    }
    let code = err.code().map(|state| state.code()).unwrap_or_default();
    if code.starts_with("08") || code.starts_with("57") {
        return true;
    }

    let msg = err.to_string();
    msg.starts_with("error connecting to server")
        || msg.starts_with("timeout waiting for server")
        || msg.starts_with("error communicating with the server")
}
