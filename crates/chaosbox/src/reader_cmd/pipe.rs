//! Nonblocking Linux connector pipes. Tokio's general stdio wrappers use
//! uncancellable blocking threads, which can outlive a revoked/expired reader.

use std::{
    fs::File,
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    io::unix::AsyncFd,
};

pub(super) struct Pipe(AsyncFd<File>);

impl Pipe {
    pub(super) fn stdio(fd: u8) -> io::Result<Self> {
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        const O_NONBLOCK: i32 = 0x800;
        // Reopen into a separate nonblocking file description. Changing flags
        // on inherited stdin/stdout would also change the connector's handles.
        let file = File::options()
            .read(fd == 0)
            .write(fd == 1)
            .custom_flags(O_NONBLOCK)
            .open(format!("/proc/self/fd/{fd}"))?;
        if !file.metadata()?.file_type().is_fifo() {
            return Err(io::Error::other(
                "reader requires connector-owned stdio pipes",
            ));
        }
        AsyncFd::new(file).map(Self)
    }
}

impl AsyncRead for Pipe {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = ready!(self.0.poll_read_ready(cx))?;
            match guard
                .try_io(|inner| io::Read::read(&mut inner.get_ref(), buf.initialize_unfilled()))
            {
                Ok(Ok(n)) => {
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_) => {}
            }
        }
    }
}

impl AsyncWrite for Pipe {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.0.poll_write_ready(cx))?;
            if let Ok(result) = guard.try_io(|inner| io::Write::write(&mut inner.get_ref(), buf)) {
                return Poll::Ready(result);
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(())) // Writes go directly to the pipe; no user-space buffer.
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(())) // The owned descriptor closes when the server drops it.
    }
}
