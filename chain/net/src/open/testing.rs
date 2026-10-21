//! Test helpers. `GatedNetwork`: a transport wrapper with an on/off switch, to cut a node off the
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

/// Test helper: a transport whose sends take `len / bytes_per_s` of (simulated) time, like a
/// link of fixed bandwidth. Bulk frames queued to a peer then really take time to drain.
pub struct ThrottledNetwork<N, C> {
    inner: N,
    clock: Arc<C>,
    bytes_per_s: f64,
}

impl<N: Clone, C> Clone for ThrottledNetwork<N, C> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), clock: self.clock.clone(), bytes_per_s: self.bytes_per_s }
    }
}

impl<N, C> ThrottledNetwork<N, C> {
    pub fn new(inner: N, clock: C, bytes_per_s: f64) -> Self {
        Self { inner, clock: Arc::new(clock), bytes_per_s }
    }
}

pub struct ThrottledListener<L, C> {
    inner: L,
    clock: Arc<C>,
    bytes_per_s: f64,
}

pub struct ThrottledSink<S, C> {
    inner: S,
    clock: Arc<C>,
    bytes_per_s: f64,
}

impl<N: Network, C: commonware_runtime::Clock + Send + Sync + 'static> Network for ThrottledNetwork<N, C> {
    type Listener = ThrottledListener<N::Listener, C>;

    async fn bind(&self, socket: SocketAddr) -> Result<Self::Listener, Error> {
        Ok(ThrottledListener { inner: self.inner.bind(socket).await?, clock: self.clock.clone(), bytes_per_s: self.bytes_per_s })
    }

    async fn dial(
        &self,
        socket: SocketAddr,
    ) -> Result<(ThrottledSink<<N::Listener as Listener>::Sink, C>, <N::Listener as Listener>::Stream), Error> {
        let (s, r) = self.inner.dial(socket).await?;
        Ok((ThrottledSink { inner: s, clock: self.clock.clone(), bytes_per_s: self.bytes_per_s }, r))
    }
}

impl<L: Listener, C: commonware_runtime::Clock + Send + Sync + 'static> Listener for ThrottledListener<L, C> {
    type Sink = ThrottledSink<L::Sink, C>;
    type Stream = L::Stream;

    async fn accept(&mut self) -> Result<(SocketAddr, Self::Sink, Self::Stream), Error> {
        let (a, s, r) = self.inner.accept().await?;
        Ok((a, ThrottledSink { inner: s, clock: self.clock.clone(), bytes_per_s: self.bytes_per_s }, r))
    }

    fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.inner.local_addr()
    }
}

impl<S: Sink, C: commonware_runtime::Clock + Send + Sync + 'static> Sink for ThrottledSink<S, C> {
    async fn send(&mut self, bufs: impl Into<IoBufs> + Send) -> Result<(), Error> {
        let bufs: IoBufs = bufs.into();
        let secs = bufs.len() as f64 / self.bytes_per_s;
        self.clock.sleep(std::time::Duration::from_secs_f64(secs)).await;
        self.inner.send(bufs).await
    }
}
