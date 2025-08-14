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
    let mut stream = stream::poll_fn(move |cx| conn.poll_message(cx).map_err(|e| panic!("{}", e)));

    while let Some(msg) = stream.next().await {
        match msg {
            Ok(AsyncMessage::Notice(raise)) => {
                callback(raise.into());
            }
            Ok(AsyncMessage::Notification(note)) => {
                callback(note.into());
            }
            Ok(_) => {
                // Some as of yet unhandled message type.
            }
            _ => {
                #[cfg(feature = "tracing")]
                tracing::error!("connection to the server is closed");
                #[cfg(not(feature = "tracing"))]
                break;
            }
        }
    }
}
