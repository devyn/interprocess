use crate::os::unix::udsocket::{
    ancwrap, c_wrappers,
    cmsg::{CmsgMut, CmsgRef},
    poll::{read_in_terms_of_vectored, write_in_terms_of_vectored},
    AsyncReadAncillary, AsyncWriteAncillary, ReadAncillarySuccess, ToUdSocketPath, UdSocket, UdSocketPath,
    UdStream as SyncUdStream,
};
use futures_core::ready;
use futures_io::{AsyncRead, AsyncWrite};
use std::{
    error::Error,
    fmt::{self, Formatter},
    io,
    net::Shutdown,
    os::{fd::AsFd, unix::net::UnixStream as StdUdStream},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead as TokioAsyncRead, AsyncWrite as TokioAsyncWrite, ReadBuf as TokioReadBuf},
    net::{unix::ReuniteError as TokioReuniteError, UnixStream as TokioUdStream},
};

mod connect_future;
mod read_half;
mod write_half;
use connect_future::*;
pub use {read_half::*, write_half::*};

/// A Unix domain socket byte stream, obtained either from [`UdStreamListener`](super::UdStreamListener) or by connecting to an existing server.
///
/// # Examples
///
/// ## Basic client
/// ```no_run
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use interprocess::os::unix::udsocket::tokio::*;
/// use tokio::{
///     io::{AsyncReadExt, AsyncWriteExt},
///     try_join,
/// };
///
/// // Await this here since we can't do a whole lot without a connection.
/// let mut conn = UdStream::connect("/tmp/example.sock").await?;
///
/// // This takes an exclusive borrow of our connection and splits it into two
/// // halves, so that we could concurrently act on both. Take care not to use
/// // the .split() method from the futures crate's AsyncReadExt.
/// let (mut reader, mut writer) = conn.split();
///
/// // Allocate a sizeable buffer for reading.
/// // This size should be enough and should be easy to find for the allocator.
/// let mut buffer = String::with_capacity(128);
///
/// // Describe the write operation as writing our whole string, waiting for
/// // that to complete, and then shutting down the write half, which sends
/// // an EOF to the other end to help it determine where the message ends.
/// let write = async {
///     writer.write_all(b"Hello from client!\n").await?;
///     writer.shutdown()?;
///     Ok(())
/// };
///
/// // Describe the read operation as reading until EOF into our big buffer.
/// let read = reader.read_to_string(&mut buffer);
///
/// // Concurrently perform both operations: write-and-send-EOF and read.
/// try_join!(write, read)?;
///
/// // Close the connection a bit earlier than you'd think we would. Nice practice!
/// drop(conn);
///
/// // Display the results when we're done!
/// println!("Server answered: {}", buffer.trim());
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct UdStream(TokioUdStream);
impl UdStream {
    /// Connects to a Unix domain socket server at the specified path.
    ///
    /// See [`ToUdSocketPath`] for an example of using various string types to specify socket paths.
    pub async fn connect(path: impl ToUdSocketPath<'_>) -> io::Result<Self> {
        let path = path.to_socket_path()?;
        Self::_connect(&path).await
    }
    async fn _connect(path: &UdSocketPath<'_>) -> io::Result<Self> {
        let stream = ConnectFuture { path }.await?;
        Self::try_from(stream).map_err(|e| e.cause.unwrap())
    }

    /// Borrows a stream into a read half and a write half, which can be used to read and write the stream concurrently.
    ///
    /// This method is more efficient than [`.into_split()`](Self::into_split), but the halves cannot be moved into independently spawned tasks.
    pub fn split(&mut self) -> (BorrowedReadHalf<'_>, BorrowedWriteHalf<'_>) {
        let (read_tok, write_tok) = self.0.split();
        (BorrowedReadHalf(read_tok), BorrowedWriteHalf(write_tok))
    }
    /// Splits a stream into a read half and a write half, which can be used to read and write the stream concurrently.
    ///
    /// Unlike [`.split()`](Self::split), the owned halves can be moved to separate tasks, which comes at the cost of a heap allocation.
    ///
    /// Dropping either half will shut it down. This is equivalent to calling [`.shutdown()`](Self::shutdown) on the stream with the corresponding argument.
    pub fn into_split(self) -> (OwnedReadHalf, OwnedWriteHalf) {
        let (read_tok, write_tok) = self.0.into_split();
        (OwnedReadHalf(read_tok), OwnedWriteHalf(write_tok))
    }
    /// Attempts to put two owned halves of a stream back together and recover the original stream. Succeeds only if the two halves originated from the same call to [`.into_split()`](Self::into_split).
    pub fn reunite(read: OwnedReadHalf, write: OwnedWriteHalf) -> Result<Self, ReuniteError> {
        let (read_tok, write_tok) = (read.0, write.0);
        let stream_tok = read_tok.reunite(write_tok)?;
        Ok(Self::from(stream_tok))
    }

    fn pinproject(self: Pin<&mut Self>) -> Pin<&mut TokioUdStream> {
        Pin::new(&mut self.get_mut().0)
    }
}
tokio_wrapper_trait_impls!(
    for UdStream,
    sync SyncUdStream,
    std StdUdStream,
    tokio TokioUdStream);
derive_asraw!(unix: UdStream);

impl TokioAsyncRead for UdStream {
    #[inline]
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut TokioReadBuf<'_>) -> Poll<io::Result<()>> {
        self.pinproject().poll_read(cx, buf)
    }
}
impl AsyncRead for UdStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut buf = TokioReadBuf::new(buf);
        match self.pinproject().poll_read(cx, &mut buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<AB: CmsgMut + ?Sized> AsyncReadAncillary<AB> for UdStream {
    #[inline]
    fn poll_read_ancillary(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
        abuf: &mut AB,
    ) -> Poll<io::Result<ReadAncillarySuccess>> {
        read_in_terms_of_vectored(self, cx, buf, abuf)
    }
    fn poll_read_ancillary_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        abuf: &mut AB,
    ) -> Poll<io::Result<ReadAncillarySuccess>> {
        let slf = self.get_mut();
        loop {
            match ancwrap::recvmsg(slf.as_fd(), bufs, abuf, None) {
                Ok(r) => return Poll::Ready(Ok(r)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
            ready!(slf.0.poll_read_ready(cx))?;
        }
    }
}

impl TokioAsyncWrite for UdStream {
    #[inline]
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.pinproject().poll_write(cx, buf)
    }
    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pinproject().poll_flush(cx)
    }
    /// Finishes immediately. See the `.shutdown()` method.
    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pinproject().poll_shutdown(cx)
    }
}
impl AsyncWrite for UdStream {
    #[inline]
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.pinproject().poll_write(cx, buf)
    }
    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.pinproject().poll_flush(cx)
    }
    /// Finishes immediately. See the `.shutdown()` method.
    #[inline]
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.shutdown(Shutdown::Both)?;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWriteAncillary for UdStream {
    #[inline]
    fn poll_write_ancillary(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
        abuf: CmsgRef<'_, '_>,
    ) -> Poll<io::Result<usize>> {
        write_in_terms_of_vectored(self, cx, buf, abuf)
    }
    fn poll_write_ancillary_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
        abuf: CmsgRef<'_, '_>,
    ) -> Poll<io::Result<usize>> {
        let slf = self.get_mut();
        loop {
            match ancwrap::sendmsg(slf.as_fd(), bufs, abuf) {
                Ok(r) => return Poll::Ready(Ok(r)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Poll::Ready(Err(e)),
            }
            ready!(slf.0.poll_write_ready(cx))?;
        }
    }
}

/// Error indicating that a read half and a write half were not from the same stream, and thus could not be reunited.
#[derive(Debug)]
pub struct ReuniteError(pub OwnedReadHalf, pub OwnedWriteHalf);
impl Error for ReuniteError {}
impl fmt::Display for ReuniteError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("tried to reunite halves of different streams")
    }
}
impl From<TokioReuniteError> for ReuniteError {
    fn from(TokioReuniteError(read, write): TokioReuniteError) -> Self {
        let read = OwnedReadHalf::from(read);
        let write = OwnedWriteHalf::from(write);
        Self(read, write)
    }
}
impl From<ReuniteError> for TokioReuniteError {
    fn from(ReuniteError(read, write): ReuniteError) -> Self {
        let read = read.into();
        let write = write.into();
        Self(read, write)
    }
}
