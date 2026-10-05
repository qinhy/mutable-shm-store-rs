use std::io::{IoSlice, IoSliceMut, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

use nix::cmsg_space;
use nix::sys::socket::{recvmsg, sendmsg, ControlMessage, ControlMessageOwned, MsgFlags};
use serde_json::Value;

use crate::error::{MStoreError, Result};

pub const MAX_FRAME: usize = 4 * 1024 * 1024;
const HEADER_SIZE: usize = 4;

pub fn encode_frame(message: &Value) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(message)?;
    if payload.len() > MAX_FRAME {
        return Err(MStoreError::Protocol(format!(
            "control frame exceeds {MAX_FRAME} bytes"
        )));
    }
    let mut frame = Vec::with_capacity(HEADER_SIZE + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<Value> {
    if frame.len() < HEADER_SIZE {
        return Err(MStoreError::Protocol(
            "control frame is missing its length header".into(),
        ));
    }
    let size = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    if size > MAX_FRAME {
        return Err(MStoreError::Protocol(format!(
            "control frame exceeds {MAX_FRAME} bytes"
        )));
    }
    if frame.len() != HEADER_SIZE + size {
        return Err(MStoreError::Protocol(
            "control frame length does not match its header".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&frame[HEADER_SIZE..])
        .map_err(|_| MStoreError::Protocol("invalid JSON control frame".into()))?;
    if !value.is_object() {
        return Err(MStoreError::Protocol(
            "control frame must be a JSON object".into(),
        ));
    }
    Ok(value)
}

pub fn send_plain<S: Write>(stream: &mut S, message: &Value) -> Result<()> {
    let frame = encode_frame(message)?;
    stream.write_all(&frame)?;
    Ok(())
}

pub fn recv_plain<S: Read>(stream: &mut S) -> Result<Value> {
    let mut header = [0u8; HEADER_SIZE];
    stream.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_FRAME {
        return Err(MStoreError::Protocol(format!(
            "control frame exceeds {MAX_FRAME} bytes"
        )));
    }
    let mut frame = Vec::with_capacity(HEADER_SIZE + size);
    frame.extend_from_slice(&header);
    frame.resize(HEADER_SIZE + size, 0);
    stream.read_exact(&mut frame[HEADER_SIZE..])?;
    decode_frame(&frame)
}

pub fn send_unix(stream: &mut UnixStream, message: &Value, fd: Option<RawFd>) -> Result<()> {
    let frame = encode_frame(message)?;
    if let Some(fd) = fd {
        let iov = [IoSlice::new(&frame)];
        let fds = [fd];
        let cmsgs = [ControlMessage::ScmRights(&fds)];
        let sent = sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsgs, MsgFlags::empty(), None)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        if sent < frame.len() {
            stream.write_all(&frame[sent..])?;
        }
        Ok(())
    } else {
        stream.write_all(&frame)?;
        Ok(())
    }
}

pub fn recv_unix(stream: &mut UnixStream) -> Result<(Value, Option<OwnedFd>)> {
    // The first recvmsg captures ancillary data. The remainder, if any, is read
    // normally from the same persistent stream.
    let mut first = vec![0u8; 64 * 1024];
    let (bytes_read, received_fd) = {
        let mut iov = [IoSliceMut::new(&mut first)];
        let mut cmsg_buffer = cmsg_space!([RawFd; 4]);
        let msg = recvmsg::<()>(
            stream.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg_buffer),
            MsgFlags::empty(),
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

        if msg.bytes == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed while receiving a control frame",
            )
            .into());
        }

        let mut received_fd: Option<OwnedFd> = None;
        for cmsg in msg
            .cmsgs()
            .map_err(|e| MStoreError::Protocol(format!("invalid ancillary data: {e}")))?
        {
            if let ControlMessageOwned::ScmRights(fds) = cmsg {
                for raw in fds {
                    if received_fd.is_none() {
                        // SAFETY: SCM_RIGHTS delivered a fresh descriptor owned by the receiver.
                        received_fd = Some(unsafe { OwnedFd::from_raw_fd(raw) });
                    } else {
                        // SAFETY: any unexpected extra received descriptor must be closed.
                        unsafe { libc::close(raw) };
                    }
                }
            }
        }
        (msg.bytes, received_fd)
    };
    first.truncate(bytes_read);

    let result = (|| -> Result<Value> {
        while first.len() < HEADER_SIZE {
            let mut rest = vec![0u8; HEADER_SIZE - first.len()];
            stream.read_exact(&mut rest)?;
            first.extend_from_slice(&rest);
        }
        let size = u32::from_be_bytes(first[..4].try_into().unwrap()) as usize;
        if size > MAX_FRAME {
            return Err(MStoreError::Protocol(format!(
                "control frame exceeds {MAX_FRAME} bytes"
            )));
        }
        let total = HEADER_SIZE + size;
        if first.len() > total {
            return Err(MStoreError::Protocol(
                "received bytes beyond the end of a control frame".into(),
            ));
        }
        if first.len() < total {
            let old_len = first.len();
            first.resize(total, 0);
            stream.read_exact(&mut first[old_len..])?;
        }
        decode_frame(&first)
    })();

    match result {
        Ok(value) => Ok((value, received_fd)),
        Err(err) => {
            drop(received_fd);
            Err(err)
        }
    }
}
