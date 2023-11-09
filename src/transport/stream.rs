use std::{
    sync::{mpsc, Mutex, Arc},
    pin::Pin,
    task::{Context, Poll, Waker},
    io,
};

use futures::{AsyncRead, AsyncWrite};

/// Local synchronous stream that pretend to be async
#[derive(Debug)]
pub struct Stream {
    tx: mpsc::Sender<Vec<u8>>,
    rx: Option<mpsc::Receiver<Vec<u8>>>,
    buf: Vec<u8>,
    offset: usize,
    // TODO: optimize
    waker_cell: Arc<Mutex<Option<Waker>>>,
}

impl Stream {
    pub(super) fn pair() -> (Self, Self) {
        let (ltx, lrx) = mpsc::channel();
        let (rtx, rrx) = mpsc::channel();
        let waker_cell = Arc::new(Mutex::new(None));

        (
            Stream {
                tx: ltx,
                rx: Some(rrx),
                buf: vec![],
                offset: 0,
                waker_cell: waker_cell.clone(),
            },
            Stream {
                tx: rtx,
                rx: Some(lrx),
                buf: vec![],
                offset: 0,
                waker_cell,
            },
        )
    }

    pub(super) fn try_recv(&mut self) -> Result<Vec<u8>, mpsc::TryRecvError> {
        let Some(rx) = &mut self.rx else {
            return Err(mpsc::TryRecvError::Disconnected);
        };
        rx.try_recv()
    }

    pub(super) fn send(&mut self, v: Vec<u8>) -> Option<()> {
        self.waker_cell
            .lock()
            .expect("poisoned")
            .take()
            .map(Waker::wake);
        self.tx.send(v).ok()
    }

    pub(super) fn mark_disconnected(&mut self) {
        self.rx = None;
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.buf.len() > self.offset {
            let len = Ord::min(self.buf.len() - self.offset, buf.len());
            let new_offset = self.offset + len;
            buf[..len].clone_from_slice(&self.buf[self.offset..new_offset]);
            self.offset = new_offset;
            return Poll::Ready(Ok(len));
        }
        if let Some(rx) = self.rx.take() {
            match rx.try_recv() {
                Ok(v) => {
                    self.rx = Some(rx);
                    self.buf = v;
                    self.offset = 0;
                    self.poll_read(cx, buf)
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.rx = Some(rx);
                    *self.waker_cell.lock().expect("poisoned") = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        } else {
            Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()))
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.tx.send(buf.to_vec()).is_err() {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        } else {
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.rx = None;
        Poll::Ready(Ok(()))
    }
}
