use std::io::{self, BufRead as _, IoSlice, Read, Write};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{Context, Poll};
use rustls::{ConnectionCommon, SideData};
use tokio::io::{AsyncRead, AsyncReadBuf, AsyncWrite, ReadBuf};

#[derive(Debug)]
pub enum ConnState {
    #[cfg(feature = "early-data")]
    EarlyData(usize, Vec<u8>),
    Stream,
    RDown,
    WDown,
    FDown,

}

impl ConnState {
    #[inline]
    pub fn rdown(&mut self) {
        match *self {
            ConnState::WDown | ConnState::FDown => *self = ConnState::FDown,
            _ => *self = ConnState::RDown,    
        }
    }

    #[inline]
    pub fn wdown(&mut self) {
        match *self {
            ConnState::RDown | ConnState::FDown => *self = ConnState::FDown,
            _ => *self = ConnState::WDown,
        }
    }

    #[inline]
    pub fn can_read(&self) -> bool {
        match *self {
            ConnState::RDown | ConnState::FDown => false,
            _ => true,
        }
    }

    #[inline]
    pub fn can_write(&self) -> bool {
        match *self {
            ConnState::WDown | ConnState::FDown => false,
            _ => true,
        }
    }

    #[inline]
    #[cfg(feature = "early-data")]
    pub fn is_early_data(&self) -> bool {
        match *self {
            ConnState::EarlyData(..) => true,
            _ => false,
        }
    }

    #[inline]
    #[cfg(not(feature = "early-data"))]
    pub const fn is_early_data(&self) -> bool {
        false
    }

}

pub struct ConnStream<'a, io, conn> {
    pub io: &'a mut io,
    pub session: &'a mut conn,
    pub end = bool,
}

impl<'a, io: Unpin + AsyncRead + AsyncWrite, conn, side> ConnStream<'a, io, conn>
where
    conn: DerefMut + Deref<Target = ConnectionCommon<side>>,
    side: SideData,
{
    pub fn new(io: &'a mut io, session: &'a mut conn) -> Self {
        Self {
            io,
            session,
            end: false,
        }
    }

    pub fn read(&mut self, cx: &mut Context) -> Poll<io::Result<usize>> {
        let mut reader = SyncReadAdapter {
            io: self.io, cx
        };

        let ret = self.session.read_tls(&mut reader);
        let x = match ret {
            Ok(x) => x,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Poll::Pending;
            }
            Err(e) => return Poll::Ready(Err(e)),
        };

        self.session.process_new_packets().map_err(|e| {
            let _ = self.write(cx);
            io::Error::new(io::ErrorKind::InvalidData, e)
        })?;

        Poll::Ready(Ok(x))
    }

    pub fn write(&mut self, cx: &mut Context) -> Poll<io::Result<usize>> {
        let mut writer = SyncWriteAdapter {
            io: self.io, cx
        };

        let ret = self.session.write_tls(&mut writer);
        let x = match ret {
            Ok(x) => Poll::Ready(Ok(x)),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                return Poll::Pending;
            }
            Err(e) => return Poll::Ready(Err(e)),
        };

    }

    pub fn handshake(&mut self, cx: &mut Context) -> Poll<io::Result<()>> {
        let mut wlen = 0;
        let mut rlen = 0;

        loop {
            let mut wblock = false;
            let mut rblock = false;
            let mut flush = false;

            while self.session.wants_write() {
                match self.write(cx) {
                    Poll::Ready(Ok(0)) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                    Poll::Ready(Ok(n)) => {
                        wlen += n;
                        flush = true;
                    }

                    Poll::Pending => {
                        wblock = true;
                        break;
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                }
            }

            if flush {
                match Pin::new(&mut self.io).flush(cx) {
                    Poll::Ready(Ok(())) => (),
                    Poll::Pending => wblock = true,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                }
            }

            while !self.end && self.session.wants_read() {
                match self.read(cx) {
                    Poll::Ready(Ok(0)) => {
                        self.end = true;
                        break;
                    }
                    Poll::Ready(Ok(n)) => rlen += n,
                    Poll::Pending => {
                        rblock = true;
                        break;
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                }
            }

            return match (self.end, self.session.is_handshaking()) {
                (true, true) => Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into())),
                (_, false) => Poll::Ready(Ok((rlen, wlen))),
                (_, true) if rblock || wblock => {
                    if rlen == 0 && wlen == 0 {
                        Poll::Pending
                    } else {
                        Poll::Ready(Ok((rlen, wlen)))
                    }
                }
                (...) => continue,
            };
        }

    }

    pub(crate) fn pending_io(mut self, cx: &mut Context<'_>) -> Poll<io::Result<&'a [u8]>> where side: 'a {
        let mut pending = false;

        while !self.end && self.session.wants_read() {
            match self.read(cx) {
                Poll::Ready(Ok(0)) => {
                    break;
                }
                Poll::Ready(Ok(_)) => (),
                Poll::Pending => { pending = true; break; }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            }
        }

        match self.session.reader().into_first_chunk() {
            Ok(buf) => Poll::Ready(Ok(buf)),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                if !pending {
                    cx.waker().wake_by_ref();
                }

                Poll::Pending
            }
            Err(err) => Poll::Ready(Err(err)),
        }
    }

    impl<'a, io: Unpin + AsyncRead + AsyncWrite, conn, side> AsyncRead for ConnStream<'a, io, conn>
    where
        conn: DerefMut + Deref<Target = ConnectionCommon<side>>,
        side: SideData,
    {
        fn poll_read(self: Pin<&mut Self>, cx: &mut Context, buf: &mut ReadBuf) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            let data = ready!(this.pending_io(cx))?;
            let rem = buf.remaining().min(data.len());
            buf.put_slice(&data[..rem]);
            self.session.reader().consume(rem);
            Poll::Ready(Ok(()))
        }
    }

    impl<'a, io: Unpin + AsyncRead + AsyncWrite, conn, side> AsyncBufRead for ConnStream<'a, io, conn>
    where
        conn: DerefMut + Deref<Target = ConnectionCommon<side>>,
        side: SideData,
    {
        fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<&[u8]>> {
            let this = self.get_mut();
            Stream {io: this.io, session: this.session}.pending_io(cx)
        }

        fn consume(self: Pin<&mut Self>, amt: usize) {
            let this = self.get_mut();
            this.session.reader().consume(amt);
        }

    }

    impl<IO: Unpin + AsyncRead + AsyncWrite, conn, side> AsyncWrite for ConnStream<'_, IO, conn>
    where
        conn: DerefMut + Deref<Target = ConnectionCommon<side>>,
        side: SideData,
    {

        fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {

            let mut i = 0;
            while i < buf.len() {
                let mut block = false;
                match self.session.writer().write(&buf[i..]) {
                    Ok(n) => i += n,
                    Err(e) => return Poll::Ready(Err(e)),
                }

                while self.session.wants_write() {
                    match self.write(cx) {
                        Poll::Ready(Ok(0)) | Poll::Pending => {
                            block = true;
                            break;
                        }
                        Poll::Ready(Ok(_)) => (),
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    }
                }

                return match (i, block) {
                    (0, true) => Poll::Pending,
                    (n, true) => Poll::Ready(Ok(n)),
                    (_, false) => continue,
                };
            }

            Poll::Ready(Ok(i))
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
            self.session.writer().flush();
            while self.session.wants_write() {
                if ready!(self.write(cx))? == 0 {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
            }
            Pin::new(&mut self.io).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            while self.session.wants_write() {
                if ready!(self.write(cx))? == 0 {
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
            }

            Poll::Ready(match ready!(Pin::new(&mut self.io).poll_shutdown(cx)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotConnected => Ok(()),
                Err(e) => Err(e),
            })
        }

    }

    pub struct SyncReadAdapter<'a, 'b, T> {
        io: &'a mut T,
        cx: &'a mut Context<'b>,
    }

    impl<T: AsyncRead + Unpin> Read for SyncReadAdapter<'_, '_, T> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut buf = ReadBuf::new(buf);
            match Pin::new(&mut self.io).poll_read(self.cx, &mut buf)? {
                Poll::Ready(Ok(())) => Ok(buf.filled().len()),
                Poll::Ready(Err(e)) => Err(e),
                Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
            }
        }
    }

    pub struct SyncWriteAdapter<'a, 'b, T> {
        io: &'a mut T,
        cx: &'a mut Context<'b>,
    }

    impl<T: Unpin> SyncWriteAdapter<'_, '_, T> {
        fn poll_with<U>(
            &mut self,
            f: impl FnOnce(&mut T, &mut Context<'_>) -> Poll<io::Result<U>>,
        ) -> io::Result<U> {
            match f(Pin::new(self.io), self.cx) {
                Poll::Ready(x) => x,
                Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
            }
        }
    }

    impl<T: AsyncWrite + Unpin> Write for SyncWriteAdapter<'_, '_, T> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.poll_with(|io, cx| io.poll_write(cx, buf))
        }

        fn flush(&mut self) -> io::Result<()> {
            self.poll_with(|io, cx| io.poll_flush(cx))
        }
    }


                    
                    

