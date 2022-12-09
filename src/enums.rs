use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug)]
pub enum JoinCmd {
    JoinAll,
    Join(String),
    Leave(String),
}

#[derive(Debug)]
pub enum IrcError {
    IoError(std::io::Error),
    BroadcastError(broadcast::error::SendError<Arc<Vec<u8>>>),
    BroadcastError2(broadcast::error::SendError<Vec<u8>>),
    MspcError(mpsc::error::SendError<Vec<u8>>),
    JoinError(mpsc::error::SendError<JoinCmd>),
    MustReconnect,
}

impl From<std::io::Error> for IrcError {
    fn from(e: std::io::Error) -> Self {
        IrcError::IoError(e)
    }
}

impl From<broadcast::error::SendError<Arc<Vec<u8>>>> for IrcError {
    fn from(e: broadcast::error::SendError<Arc<Vec<u8>>>) -> Self {
        IrcError::BroadcastError(e)
    }
}

impl From<broadcast::error::SendError<Vec<u8>>> for IrcError {
    fn from(e: broadcast::error::SendError<Vec<u8>>) -> Self {
        IrcError::BroadcastError2(e)
    }
}

impl From<mpsc::error::SendError<Vec<u8>>> for IrcError {
    fn from(e: mpsc::error::SendError<Vec<u8>>) -> Self {
        IrcError::MspcError(e)
    }
}

impl From<mpsc::error::SendError<JoinCmd>> for IrcError {
    fn from(e: mpsc::error::SendError<JoinCmd>) -> Self {
        IrcError::JoinError(e)
    }
}

impl std::fmt::Display for IrcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IrcError::IoError(e) => write!(f, "{}", e),
            IrcError::BroadcastError(e) => write!(f, "{}", e),
            IrcError::BroadcastError2(e) => write!(f, "{}", e),
            IrcError::MspcError(e) => write!(f, "{}", e),
            IrcError::JoinError(e) => write!(f, "{}", e),
            IrcError::MustReconnect => write!(f, "IrcError::MustReconnect"),
        }
    }
}
