use std::io;
use std::net::SocketAddr;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("cannot listen on {addr}: {source}")]
    Bind { addr: SocketAddr, source: io::Error },
    #[error("invalid stored state: {0}")]
    State(String),
}
