//! Test helper: a transport wrapper with an on/off switch, to cut a node off the
//! network (partition) and heal it. While `down`: dials fail, accepted connections are
//! dropped, and sends / receives on existing links fail (so they close).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use commonware_runtime::{Error, IoBufs, Listener, Network, Sink, Stream};

#[derive(Clone)]
pub struct GatedNetwork<N> {
    inner: N,
    down: Arc<AtomicBool>,
}

impl<N> GatedNetwork<N> {
    pub fn new(inner: N) -> (Self, Arc<AtomicBool>) {
        let down = Arc::new(AtomicBool::new(false));
        (Self { inner, down: down.clone() }, down)
    }
}

pub struct GatedListener<L> {
    inner: L,
    down: Arc<AtomicBool>,
}

pub struct GatedSink<S> {
    inner: S,
    down: Arc<AtomicBool>,
}

pub struct GatedStream<S> {
    inner: S,
    down: Arc<AtomicBool>,
}

impl<N: Network> Network for GatedNetwork<N> {
    type Listener = GatedListener<N::Listener>;

    async fn bind(&self, socket: SocketAddr) -> Result<Self::Listener, Error> {
        Ok(GatedListener { inner: self.inner.bind(socket).await?, down: self.down.clone() })
    }

    async fn dial(
        &self,
        socket: SocketAddr,
    ) -> Result<(GatedSink<<N::Listener as Listener>::Sink>, GatedStream<<N::Listener as Listener>::Stream>), Error> {
        if self.down.load(Ordering::SeqCst) {
            return Err(Error::ConnectionFailed);
        }
        let (s, r) = self.inner.dial(socket).await?;
        Ok((GatedSink { inner: s, down: self.down.clone() }, GatedStream { inner: r, down: self.down.clone() }))
    }
}

impl<L: Listener> Listener for GatedListener<L> {
    type Sink = GatedSink<L::Sink>;
    type Stream = GatedStream<L::Stream>;

    async fn accept(&mut self) -> Result<(SocketAddr, Self::Sink, Self::Stream), Error> {
        loop {
            let (a, s, r) = self.inner.accept().await?;
            if self.down.load(Ordering::SeqCst) {
                continue;
            }
            return Ok((a, GatedSink { inner: s, down: self.down.clone() }, GatedStream { inner: r, down: self.down.clone() }));
        }
    }

    fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.inner.local_addr()
    }
}

impl<S: Sink> Sink for GatedSink<S> {
    async fn send(&mut self, bufs: impl Into<IoBufs> + Send) -> Result<(), Error> {
        if self.down.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        self.inner.send(bufs).await
    }
}

impl<S: Stream> Stream for GatedStream<S> {
    async fn recv(&mut self, len: usize) -> Result<IoBufs, Error> {
        if self.down.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        let r = self.inner.recv(len).await;
        if self.down.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        r
    }

    fn peek(&self, max_len: usize) -> &[u8] {
        if self.down.load(Ordering::SeqCst) { &[] } else { self.inner.peek(max_len) }
    }
}
