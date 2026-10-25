// SPDX-License-Identifier: Apache-2.0
//! Keep stdin reads interruptible without closing a descriptor owned by another thread.
use std::{
    io::{self, BufRead, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub struct Input {
    chunks: Option<mpsc::Receiver<io::Result<Vec<u8>>>>,
    cancelled: Arc<AtomicBool>,
    buffer: Vec<u8>,
    offset: usize,
    eof: bool,
}
impl Input {
    pub const fn stdin(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            chunks: None,
            cancelled,
            buffer: vec![],
            offset: 0,
            eof: false,
        }
    }
}
fn reader() -> mpsc::Receiver<io::Result<Vec<u8>>> {
    let (send, chunks) = mpsc::sync_channel(1);
    // This single detached reader can remain blocked until process exit. The
    // command thread never waits for it during signal-driven shutdown.
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            let mut bytes = vec![0; 8192];
            match stdin.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    if send.send(Ok(bytes)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                    break;
                }
            }
        }
    });
    chunks
}

impl BufRead for Input {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.offset == self.buffer.len() && !self.eof {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::other("Request cancelled."));
            }
            match self
                .chunks
                .get_or_insert_with(reader)
                .recv_timeout(Duration::from_millis(25))
            {
                Ok(bytes) => {
                    self.buffer = bytes?;
                    self.offset = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => self.eof = true,
            }
        }
        Ok(&self.buffer[self.offset..])
    }
    fn consume(&mut self, amount: usize) {
        self.offset = (self.offset + amount).min(self.buffer.len());
    }
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let count = bytes.len().min(available.len());
        bytes[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}
