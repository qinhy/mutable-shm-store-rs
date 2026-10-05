use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::error::{MStoreError, Result};
use crate::protocol::{recv_plain, recv_unix, send_plain, send_unix};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

pub fn parse_endpoint(endpoint: &str) -> Result<Endpoint> {
    if let Some(path) = endpoint.strip_prefix("unix://") {
        if path.is_empty() {
            return Err(MStoreError::InvalidRequest(
                "unix endpoint requires a path".into(),
            ));
        }
        return Ok(Endpoint::Unix(PathBuf::from(path)));
    }
    if let Some(address) = endpoint.strip_prefix("tcp://") {
        let mut resolved = address.to_socket_addrs().map_err(|e| {
            MStoreError::InvalidRequest(format!("invalid TCP endpoint {endpoint:?}: {e}"))
        })?;
        let addr = resolved.next().ok_or_else(|| {
            MStoreError::InvalidRequest(format!("TCP endpoint did not resolve: {endpoint}"))
        })?;
        return Ok(Endpoint::Tcp(addr));
    }
    Err(MStoreError::InvalidRequest(
        "endpoint must start with unix:// or tcp://".into(),
    ))
}

pub enum ControlConnection {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl ControlConnection {
    pub fn send(&mut self, message: &Value, fd: Option<&OwnedFd>) -> Result<()> {
        match self {
            Self::Unix(stream) => send_unix(stream, message, fd.map(|value| value.as_raw_fd())),
            Self::Tcp(stream) => {
                if fd.is_some() {
                    return Err(MStoreError::Protocol(
                        "file descriptors cannot be passed over TCP".into(),
                    ));
                }
                send_plain(stream, message)
            }
        }
    }

    pub fn recv(&mut self) -> Result<(Value, Option<OwnedFd>)> {
        match self {
            Self::Unix(stream) => recv_unix(stream),
            Self::Tcp(stream) => Ok((recv_plain(stream)?, None)),
        }
    }

    pub fn set_timeout(&self, timeout: Duration) -> io::Result<()> {
        match self {
            Self::Unix(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))
            }
            Self::Tcp(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))
            }
        }
    }
}

pub fn connect_control(endpoint: &str, timeout: Duration) -> Result<ControlConnection> {
    let conn = match parse_endpoint(endpoint)? {
        Endpoint::Unix(path) => ControlConnection::Unix(UnixStream::connect(path)?),
        Endpoint::Tcp(addr) => {
            let stream = TcpStream::connect_timeout(&addr, timeout)?;
            stream.set_nodelay(true)?;
            ControlConnection::Tcp(stream)
        }
    };
    conn.set_timeout(timeout)?;
    Ok(conn)
}

pub enum Listener {
    Unix(UnixListener, PathBuf),
    Tcp(TcpListener),
}

impl Listener {
    pub fn bind(endpoint: &str) -> Result<(Self, String)> {
        match parse_endpoint(endpoint)? {
            Endpoint::Unix(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
                let listener = UnixListener::bind(&path)?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                listener.set_nonblocking(true)?;
                Ok((
                    Self::Unix(listener, path.clone()),
                    format!("unix://{}", path.display()),
                ))
            }
            Endpoint::Tcp(addr) => {
                let listener = TcpListener::bind(addr)?;
                listener.set_nonblocking(true)?;
                let published = format!("tcp://{}", listener.local_addr()?);
                Ok((Self::Tcp(listener), published))
            }
        }
    }

    pub fn accept(&self) -> io::Result<ControlConnection> {
        match self {
            Self::Unix(listener, _) => {
                let (stream, _) = listener.accept()?;
                stream.set_nonblocking(false)?;
                Ok(ControlConnection::Unix(stream))
            }
            Self::Tcp(listener) => {
                let (stream, _) = listener.accept()?;
                stream.set_nonblocking(false)?;
                stream.set_nodelay(true)?;
                Ok(ControlConnection::Tcp(stream))
            }
        }
    }

    pub fn cleanup(&self) {
        if let Self::Unix(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn unix_path(&self) -> Option<&Path> {
        match self {
            Self::Unix(_, path) => Some(path),
            Self::Tcp(_) => None,
        }
    }
}
