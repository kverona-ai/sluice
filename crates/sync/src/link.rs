//! A bidirectional byte link (LAN socket or relay session) split into the two
//! halves the session threads need. Both ends of the channel use it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::relay::{RelayConn, RelayIo};

pub struct Link {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    closer: Arc<dyn Fn() + Send + Sync>,
    /// "lan" | "relay"
    pub via: &'static str,
    set_timeout: Arc<dyn Fn(Option<Duration>) + Send + Sync>,
}

impl Link {
    pub fn tcp(stream: TcpStream) -> Result<Self> {
        stream.set_nodelay(true).ok();
        let reader = stream.try_clone()?;
        let closer_s = stream.try_clone()?;
        let timeout_s = stream.try_clone()?;
        Ok(Self {
            reader: Box::new(reader),
            writer: Box::new(stream),
            closer: Arc::new(move || {
                let _ = closer_s.shutdown(std::net::Shutdown::Both);
            }),
            via: "lan",
            set_timeout: Arc::new(move |d| {
                let _ = timeout_s.set_read_timeout(d);
            }),
        })
    }

    pub fn relay(conn: RelayConn) -> Result<Self> {
        let w = conn.try_clone()?;
        let c = conn.try_clone()?;
        let t = conn.try_clone()?;
        Ok(Self {
            reader: Box::new(RelayIo::new(conn)),
            writer: Box::new(RelayIo::new(w)),
            closer: Arc::new(move || c.shutdown()),
            via: "relay",
            set_timeout: Arc::new(move |d| {
                let _ = t.set_read_timeout(d);
            }),
        })
    }

    pub fn closer(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.closer.clone()
    }

    pub fn set_read_timeout(&self, d: Option<Duration>) {
        (self.set_timeout)(d)
    }
}
