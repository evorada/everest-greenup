//! A board connection: a serial device or `tcp:host:port`.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Something we can read from on one thread and write to on another.
pub(crate) trait PortIo: Read + Write + Send {
    /// A second handle on the same connection, for the reader thread.
    fn try_clone_box(&self) -> io::Result<Box<dyn PortIo>>;
    /// Make a blocked reader give up.
    fn shutdown(&self);
}

struct Tcp(TcpStream);

impl Read for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Tcp {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl PortIo for Tcp {
    fn try_clone_box(&self) -> io::Result<Box<dyn PortIo>> {
        Ok(Box::new(Tcp(self.0.try_clone()?)))
    }

    fn shutdown(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

struct Serial(Box<dyn serialport::SerialPort>);

impl Read for Serial {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Serial {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl PortIo for Serial {
    fn try_clone_box(&self) -> io::Result<Box<dyn PortIo>> {
        Ok(Box::new(Serial(self.0.try_clone()?)))
    }

    fn shutdown(&self) {}
}

/// Open `spec`: `tcp:host:port` or a device path at 115200 8N1.
///
/// `read_timeout` bounds every read so the reader thread can notice a
/// stop request.
pub(crate) fn open(spec: &str, read_timeout: Duration) -> io::Result<Box<dyn PortIo>> {
    if let Some(addr) = spec.strip_prefix("tcp:") {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(read_timeout))?;
        stream.set_nodelay(true)?;
        return Ok(Box::new(Tcp(stream)));
    }
    let port = serialport::new(spec, 115_200)
        .timeout(read_timeout)
        .open()
        .map_err(io::Error::from)?;
    Ok(Box::new(Serial(port)))
}
