use {
    crate::PGMessage,
    futures::{StreamExt, stream},
    tokio::io::{AsyncRead, AsyncWrite},
    tokio_postgres::{AsyncMessage, Connection as PGConnection},
};

///
/// Polls the connection side of the (client, connection) pair returned by the connect function.
/// Then forwards any messages received using the provided channel.
///
pub(crate) async fn handle_connection_polling<S, T>(
    mut conn: PGConnection<S, T>,
    callback: impl Fn(PGMessage) + Send + Sync + 'static,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let mut stream = stream::poll_fn(move |cx| conn.poll_message(cx));

    loop {
        match stream.next().await {
            Some(Ok(AsyncMessage::Notice(raise))) => {
                callback(raise.into());
            }
            Some(Ok(AsyncMessage::Notification(note))) => {
                callback(note.into());
            }
            Some(Ok(_)) => {
                // Unhandled message type
            }
            Some(Err(e)) => {
                #[cfg(feature = "tracing")]
                tracing::error!("Connection polling error: {}", e);
                callback(PGMessage::disconnected(e.to_string()));
                break;
            }
            None => {
                // Stream ended - connection closed normally
                #[cfg(feature = "tracing")]
                tracing::info!("Connection closed");
                callback(PGMessage::disconnected("connection closed"));
                break;
            }
        }
    }
}
