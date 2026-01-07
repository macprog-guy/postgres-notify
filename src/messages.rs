#[cfg(feature = "chrono")]
use chrono::{DateTime, SecondsFormat, Utc};
#[cfg(not(feature = "chrono"))]
use std::time::SystemTime;

#[cfg(feature = "chrono")]
pub type Timestamp = DateTime<Utc>;
#[cfg(not(feature = "chrono"))]
pub type Timestamp = SystemTime;

use {
    std::{
        fmt::{self, Display},
        str::FromStr,
        time::Duration,
    },
    tokio_postgres::error::DbError,
};

///
/// Type used to represent any of the messages that can be received
/// by the client callback.
///
#[derive(Debug, Clone)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
pub enum PGMessage {
    Notify(PGNotifyMessage),
    Raise(PGRaiseMessage),
    Reconnect {
        timestamp: Timestamp,
        attempts: u32,
        max_attempts: u32,
    },
    Connected {
        timestamp: Timestamp,
    },
    Timeout {
        timestamp: Timestamp,
        duration: Duration,
    },
    Cancelled {
        timestamp: Timestamp,
        success: bool,
    },
    FailedToReconnect {
        timestamp: Timestamp,
        attempts: u32,
    },
    Disconnected {
        timestamp: Timestamp,
        reason: String,
    },
}

impl PGMessage {
    pub fn reconnect(attempts: u32, max_attempts: u32) -> Self {
        Self::Reconnect {
            timestamp: current_timestamp(),
            attempts,
            max_attempts,
        }
    }

    pub fn connected() -> Self {
        Self::Connected {
            timestamp: current_timestamp(),
        }
    }

    pub fn timeout(duration: Duration) -> Self {
        Self::Timeout {
            timestamp: current_timestamp(),
            duration,
        }
    }

    pub fn cancelled(success: bool) -> Self {
        Self::Cancelled {
            timestamp: current_timestamp(),
            success,
        }
    }
    pub fn failed_to_reconnect(attempts: u32) -> Self {
        Self::FailedToReconnect {
            timestamp: current_timestamp(),
            attempts,
        }
    }

    pub fn disconnected(reason: impl Into<String>) -> Self {
        Self::Disconnected {
            timestamp: current_timestamp(),
            reason: reason.into(),
        }
    }
}

impl Display for PGMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use PGMessage::*;
        match self {
            Notify(m) => m.fmt(f),
            Raise(m) => m.fmt(f),
            Reconnect {
                timestamp,
                attempts,
                max_attempts,
            } => {
                let ts = format_timestamp(*timestamp);
                if *max_attempts != u32::MAX {
                    write!(
                        f,
                        "{}{:>12}: attempt #{} of {}",
                        &ts, "RECONNECT", attempts, max_attempts
                    )
                } else {
                    write!(f, "{}{:>12}: attempt #{}", &ts, "RECONNECT", attempts)
                }
            }
            Connected { timestamp } => {
                let ts = format_timestamp(*timestamp);
                write!(f, "{}{:>12}: connection established", &ts, "CONNECTED")
            }
            Timeout {
                timestamp,
                duration,
            } => {
                let ts = format_timestamp(*timestamp);
                write!(
                    f,
                    "{}{:>12}: timeout after {}ms",
                    &ts,
                    "TIMEOUT",
                    duration.as_millis(),
                )
            }
            Cancelled { timestamp, success } => {
                let ts = format_timestamp(*timestamp);
                let could = if *success { "" } else { "could not be " };
                write!(
                    f,
                    "{}{:>12}: server-side query {}cancelled",
                    &ts, "CANCELLED", could
                )
            }
            FailedToReconnect {
                timestamp,
                attempts,
            } => {
                let ts = format_timestamp(*timestamp);
                write!(
                    f,
                    "{}{:>12}: failed to reconnect after {} attempts",
                    &ts, "FAILURE", attempts
                )
            }
            Disconnected { timestamp, reason } => {
                let ts = format_timestamp(*timestamp);
                write!(f, "{}{:>12}: {}", &ts, "DISCONNECTED", reason)
            }
        }
    }
}

impl From<tokio_postgres::Notification> for PGMessage {
    fn from(note: tokio_postgres::Notification) -> Self {
        Self::Notify(note.into())
    }
}

impl From<tokio_postgres::error::DbError> for PGMessage {
    fn from(raise: tokio_postgres::error::DbError) -> Self {
        Self::Raise(raise.into())
    }
}

///
/// Message received when a `NOTIFY [channel] [payload]` is issued on PostgreSQL.
///
/// Postgres notifications contain a string payload on a named channel.
/// It also specifies the server-side process id of the notifying process.
/// To this we add a timestamp, which is either an [`chrono::DateTime<Utc>`]
/// or [`std::time::SystemTime`] depending on the `chrono` feature flag.
///
/// More details on the postgres notification can be found
/// [here](https://www.postgresql.org/docs/current/sql-notify.html).
///
#[derive(Debug, Clone)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
pub struct PGNotifyMessage {
    pub timestamp: Timestamp,
    pub process_id: i32,
    pub channel: String,
    pub payload: String,
}

impl From<tokio_postgres::Notification> for PGNotifyMessage {
    fn from(note: tokio_postgres::Notification) -> Self {
        Self {
            timestamp: current_timestamp(),
            process_id: note.process_id(),
            channel: note.channel().into(),
            payload: note.payload().into(),
        }
    }
}

impl Display for PGNotifyMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = format_timestamp(self.timestamp);
        write!(
            f,
            "{}{:>12}: pid={} sent {}={}",
            &ts, "NOTIFY", self.process_id, &self.channel, &self.payload
        )
    }
}

///
/// # Message received when a `raise <level> <message>` is issued on PostgreSQL.
///
/// Postgres logs are created by issuing `RAISE <level> <message>` commands
/// within your functions, stored procedures and scripts. When such a command is
/// issued, [`PGNotifyingClient`] receives a notification even if the call is in
/// progress, which allows the caller to capture the execution log in realtime.
///
/// Here we extract the level and message fields but all of the detailed
/// location information can be found in the `details` field.
///
/// More details on how to raise log messages can be found
/// (here)[https://www.postgresql.org/docs/current/plpgsql-errors-and-messages.html].
///
#[derive(Debug, Clone)]
#[cfg_attr(any(feature = "serde", test), derive(serde::Serialize))]
pub struct PGRaiseMessage {
    pub timestamp: Timestamp,
    pub level: PGRaiseLevel,
    pub message: String,
    #[cfg_attr(any(feature = "serde", test), serde(skip))]
    pub details: DbError,
}

impl From<DbError> for PGRaiseMessage {
    fn from(raise: DbError) -> Self {
        PGRaiseMessage {
            timestamp: current_timestamp(),
            level: PGRaiseLevel::from_str(raise.severity()).unwrap_or(PGRaiseLevel::Error),
            message: raise.message().into(),
            details: raise,
        }
    }
}

impl Display for PGRaiseMessage {
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

        write!(f, "{}{:>12}: {}", &ts, &self.level.as_ref(), self.message)
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

impl fmt::Display for PGRaiseLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_ref())
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

///
/// Returns the current time either as a DateTime<Utc> or SystemTime
/// depending on the `chrono` feature flag.
///
#[inline(always)]
fn current_timestamp() -> Timestamp {
    #[cfg(feature = "chrono")]
    return Utc::now();
    #[cfg(not(feature = "chrono"))]
    return SystemTime::now();
}

fn format_timestamp(ts: Timestamp) -> String {
    #[cfg(feature = "chrono")]
    return ts.to_rfc3339_opts(SecondsFormat::Millis, true);

    #[cfg(not(feature = "chrono"))]
    {
        let duration = ts
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let millis = duration.as_millis();
        return format!("{}", millis);
    }
}
