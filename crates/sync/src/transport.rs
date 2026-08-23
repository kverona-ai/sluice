//! Length-prefixed frames over TCP — the same framing is used on the LAN path and
//! through the relay (which only ever forwards opaque, encrypted frames).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use crate::crypto::MAX_FRAME;

pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_FRAME {
        bail!("frame too large ({} bytes)", payload.len());
    }
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

pub fn read_frame(r: &mut impl Read) -> Result<Vec<u8>> {
    read_frame_max(r, MAX_FRAME)
}

/// Like `read_frame` but with a tighter bound — used for the unauthenticated first
/// frame so a stranger on the LAN cannot make the desktop allocate 8 MiB per connection.
pub fn read_frame_max(r: &mut impl Read, max: usize) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).context("connection closed")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        bail!("frame too large ({len} bytes)");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).context("connection closed mid-frame")?;
    Ok(buf)
}

/// Connect with a bounded timeout to the first address that answers.
pub fn connect_timeout(addr: &str, timeout: Duration) -> Result<TcpStream> {
    let addrs: Vec<SocketAddr> = addr
        .to_socket_addrs()
        .with_context(|| format!("cannot resolve {addr}"))?
        .collect();
    let mut last = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                s.set_nodelay(true).ok();
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::anyhow!(
        "cannot connect to {addr}: {}",
        last.map(|e| e.to_string()).unwrap_or_else(|| "no address".into())
    ))
}

/// Best-effort list of LAN addresses this host can be reached at (IPv4, non-loopback).
pub fn lan_addresses() -> Vec<std::net::IpAddr> {
    let mut out = Vec::new();
    // Ask the OS which source address it would use for an outbound route — no
    // packet is sent for UDP "connect". Works without listing interfaces.
    for probe in [
        "10.255.255.255:9",
        "192.168.255.255:9",
        "172.31.255.255:9",
        "1.1.1.1:9",
    ] {
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0")
            && sock.connect(probe).is_ok()
            && let Ok(local) = sock.local_addr()
        {
            let ip = local.ip();
            if !ip.is_loopback() && !ip.is_unspecified() && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"abc").unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut r = std::io::Cursor::new(buf);
        assert_eq!(read_frame(&mut r).unwrap(), b"abc");
        assert_eq!(read_frame(&mut r).unwrap(), b"");
        assert!(read_frame(&mut r).is_err());
    }
}
