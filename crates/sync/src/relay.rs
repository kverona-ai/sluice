//! Minimal relay for the fallback path (05 §6.3): two parties meet in a room
//! named after the desktop id and the relay pipes **opaque** frames between them.
//! It never holds a key — every data frame is already end-to-end encrypted by the
//! channel (`crypto.rs`), so a relay on an untrusted host learns only timing and
//! sizes. Ship-with-the-app (`sluice relay serve`) or self-host anywhere.
//!
//! Outer frame layout (inside the length prefix of `transport.rs`):
//! `0x00` + JSON control (`join`, `ok`, `error`, `ka`) — handled by the relay;
//! `0x01` + bytes — forwarded verbatim to the peer.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::transport::{connect_timeout, read_frame, write_frame};

const CTRL: u8 = 0;
const DATA: u8 = 1;

/// Client side of the relay protocol; wraps a TCP stream to the relay.
pub struct RelayConn {
    stream: TcpStream,
}

impl RelayConn {
    /// Connect and join `room` as `host` (waits for a guest) or `guest`.
    pub fn join(relay_addr: &str, room: &str, host: bool, timeout: Duration) -> Result<Self> {
        let mut stream = connect_timeout(relay_addr, timeout)?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        let mut f = vec![CTRL];
        f.extend_from_slice(
            json!({"join": room, "role": if host { "host" } else { "guest" }})
                .to_string()
                .as_bytes(),
        );
        write_frame(&mut stream, &f)?;
        let ack = read_frame(&mut stream)?;
        match ack.first() {
            Some(&CTRL) => {
                let v: Value = serde_json::from_slice(&ack[1..]).unwrap_or(Value::Null);
                if let Some(e) = v.get("error").and_then(Value::as_str) {
                    bail!("relay: {e}");
                }
            }
            _ => bail!("relay: unexpected reply"),
        }
        stream.set_read_timeout(None)?;
        Ok(Self { stream })
    }

    pub fn try_clone(&self) -> Result<Self> {
        Ok(Self {
            stream: self.stream.try_clone()?,
        })
    }

    pub fn set_read_timeout(&self, d: Option<Duration>) -> Result<()> {
        Ok(self.stream.set_read_timeout(d)?)
    }

    pub fn send(&mut self, data: &[u8]) -> Result<()> {
        let mut f = Vec::with_capacity(data.len() + 1);
        f.push(DATA);
        f.extend_from_slice(data);
        write_frame(&mut self.stream, &f)
    }

    /// Tell the relay we are alive (swallowed, never forwarded).
    pub fn keepalive(&mut self) -> Result<()> {
        write_frame(
            &mut self.stream,
            &[CTRL, b'{', b'"', b'k', b'a', b'"', b':', b'1', b'}'],
        )
    }

    /// Next data frame from the peer (control frames are consumed; errors surface).
    pub fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            let f = read_frame(&mut self.stream)?;
            match f.first() {
                Some(&DATA) => return Ok(f[1..].to_vec()),
                Some(&CTRL) => {
                    let v: Value = serde_json::from_slice(&f[1..]).unwrap_or(Value::Null);
                    if let Some(e) = v.get("error").and_then(Value::as_str) {
                        bail!("relay: {e}");
                    }
                    if v.get("bye").is_some() {
                        bail!("relay: peer left");
                    }
                }
                _ => bail!("relay: malformed frame"),
            }
        }
    }

    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Whether a `recv` error was only the read timeout elapsing (socket still fine).
pub fn is_timeout(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .map(|io| {
                matches!(
                    io.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
            })
            .unwrap_or(false)
    })
}

/// A `Read + Write` adapter so the channel code can treat a relay session like a socket.
pub struct RelayIo {
    conn: RelayConn,
    pending: Vec<u8>,
    out: Vec<u8>,
}

impl RelayIo {
    pub fn new(conn: RelayConn) -> Self {
        Self {
            conn,
            pending: Vec::new(),
            out: Vec::new(),
        }
    }
    pub fn conn(&self) -> &RelayConn {
        &self.conn
    }
}

impl Read for RelayIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            let data = self
                .conn
                .recv()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionAborted, e.to_string()))?;
            // re-frame so `read_frame` on top sees a length prefix again
            self.pending.extend_from_slice(&(data.len() as u32).to_be_bytes());
            self.pending.extend_from_slice(&data);
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

impl Write for RelayIo {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // The channel writes `len || payload` in two calls; buffer until complete.
        self.pending_out().extend_from_slice(buf);
        let mut flushed = 0;
        loop {
            let out = self.pending_out();
            if out.len() < 4 {
                break;
            }
            let len = u32::from_be_bytes([out[0], out[1], out[2], out[3]]) as usize;
            if out.len() < 4 + len {
                break;
            }
            let frame: Vec<u8> = out[4..4 + len].to_vec();
            out.drain(..4 + len);
            self.conn
                .send(&frame)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e.to_string()))?;
            flushed += 1;
        }
        let _ = flushed;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RelayIo {
    fn pending_out(&mut self) -> &mut Vec<u8> {
        // separate buffer from `pending` (inbound)
        &mut self.out
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// A parked host: the guest handler hands its stream over through this channel, so
/// exactly one thread ever reads from the host socket.
type Waiting = Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<TcpStream>>>>;

/// Run a relay on `listen` until `stop` is set. Returns the bound address; the
/// accept loop runs on its own thread.
pub fn serve(listen: &str, stop: Arc<AtomicBool>) -> Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(listen).with_context(|| format!("binding relay on {listen}"))?;
    let addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let waiting: Waiting = Arc::new(Mutex::new(HashMap::new()));
    std::thread::Builder::new()
        .name("sluice-relay".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let waiting = waiting.clone();
                        std::thread::spawn(move || {
                            let _ = handle(stream, waiting);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
        })?;
    Ok(addr)
}

fn ctrl(stream: &mut TcpStream, v: Value) -> Result<()> {
    let mut f = vec![CTRL];
    f.extend_from_slice(v.to_string().as_bytes());
    write_frame(stream, &f)
}

fn handle(mut stream: TcpStream, waiting: Waiting) -> Result<()> {
    stream.set_nonblocking(false)?; // inherited from the non-blocking listener on BSD/macOS
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let first = read_frame(&mut stream)?;
    if first.first() != Some(&CTRL) {
        return ctrl(&mut stream, json!({"error": "expected join"}));
    }
    let v: Value = serde_json::from_slice(&first[1..]).unwrap_or(Value::Null);
    let room = v.get("join").and_then(Value::as_str).unwrap_or("").to_string();
    let role = v.get("role").and_then(Value::as_str).unwrap_or("");
    if room.is_empty() || room.len() > 128 {
        return ctrl(&mut stream, json!({"error": "bad room"}));
    }
    match role {
        "host" => {
            let (tx, rx) = std::sync::mpsc::channel::<TcpStream>();
            // Replace any previous (probably dead) host for this room: dropping its
            // sender makes that thread's recv fail and it exits.
            waiting.lock().unwrap().insert(room.clone(), tx);
            ctrl(&mut stream, json!({"ok": true}))?;
            stream.set_read_timeout(None)?;
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(guest) => {
                        return pump(stream, guest);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = ctrl(
                            &mut stream,
                            json!({"error": "replaced by a newer host connection"}),
                        );
                        return Ok(());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Still parked: drain keepalives, detect a dead host.
                        stream.set_nonblocking(true)?;
                        let mut peek = [0u8; 1];
                        match stream.peek(&mut peek) {
                            Ok(0) => {
                                waiting.lock().unwrap().remove(&room);
                                return Ok(());
                            }
                            Ok(_) => {
                                stream.set_nonblocking(false)?;
                                match read_frame(&mut stream) {
                                    Ok(f) if f.first() == Some(&CTRL) => {}
                                    _ => {
                                        waiting.lock().unwrap().remove(&room);
                                        return Ok(());
                                    }
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                stream.set_nonblocking(false)?;
                            }
                            Err(_) => {
                                waiting.lock().unwrap().remove(&room);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }
        "guest" => {
            let host = waiting.lock().unwrap().remove(&room);
            let Some(host) = host else {
                return ctrl(
                    &mut stream,
                    json!({"error": "desktop is not connected to the relay"}),
                );
            };
            ctrl(&mut stream, json!({"ok": true}))?;
            stream.set_read_timeout(None)?;
            let _ = host.send(stream); // host thread gone → guest simply closes
            Ok(())
        }
        _ => ctrl(&mut stream, json!({"error": "bad role"})),
    }
}

/// Forward data frames both ways until either side closes; then close both.
fn pump(a: TcpStream, b: TcpStream) -> Result<()> {
    let (mut a_r, mut a_w) = (a.try_clone()?, a);
    let (mut b_r, mut b_w) = (b.try_clone()?, b);
    let t = std::thread::spawn(move || {
        let _ = pipe(&mut a_r, &mut b_w);
        let _ = ctrl(&mut b_w, json!({"bye": true}));
        let _ = b_w.shutdown(std::net::Shutdown::Both);
    });
    let _ = pipe(&mut b_r, &mut a_w);
    let _ = ctrl(&mut a_w, json!({"bye": true}));
    let _ = a_w.shutdown(std::net::Shutdown::Both);
    let _ = t.join();
    Ok(())
}

fn pipe(from: &mut TcpStream, to: &mut TcpStream) -> Result<()> {
    loop {
        let f = read_frame(from)?;
        match f.first() {
            Some(&DATA) => write_frame(to, &f)?,
            Some(&CTRL) => {} // keepalives are not forwarded
            _ => bail!("malformed"),
        }
    }
}
