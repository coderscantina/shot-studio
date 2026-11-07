//! Copies what the server writes to the client into a log file, for auditing what an agent saw.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::AsyncWrite;

pub struct Tee<W> {
    inner: W,
    log: Option<File>,
}

impl<W> Tee<W> {
    /// Appends to `log` when given.
    pub fn new(inner: W, log: Option<&Path>) -> io::Result<Self> {
        let log = log
            .map(|path| OpenOptions::new().create(true).append(true).open(path))
            .transpose()?;
        Ok(Self { inner, log })
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for Tee<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let (Poll::Ready(Ok(n)), Some(log)) = (&written, &mut this.log) {
            log.write_all(&buf[..*n])?;
        }
        written
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
