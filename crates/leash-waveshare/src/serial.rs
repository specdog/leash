use std::{
    fmt,
    io::{self, Read, Write},
    time::Duration,
};

use crate::{ControllerIo, ControllerIoFactory};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortFactory {
    device: Box<str>,
    baud: u32,
    timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialFactoryError {
    EmptyDevice,
    ZeroBaud,
    ZeroTimeout,
}

impl fmt::Display for SerialFactoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDevice => formatter.write_str("serial device cannot be empty"),
            Self::ZeroBaud => formatter.write_str("serial baud must be positive"),
            Self::ZeroTimeout => formatter.write_str("serial timeout must be positive"),
        }
    }
}

impl std::error::Error for SerialFactoryError {}

impl SerialPortFactory {
    pub fn new(
        device: impl Into<Box<str>>,
        baud: u32,
        timeout: Duration,
    ) -> Result<Self, SerialFactoryError> {
        let device = device.into();
        if device.trim().is_empty() {
            return Err(SerialFactoryError::EmptyDevice);
        }
        if baud == 0 {
            return Err(SerialFactoryError::ZeroBaud);
        }
        if timeout.is_zero() {
            return Err(SerialFactoryError::ZeroTimeout);
        }
        Ok(Self {
            device,
            baud,
            timeout,
        })
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub const fn baud(&self) -> u32 {
        self.baud
    }

    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl ControllerIoFactory for SerialPortFactory {
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>> {
        serialport::new(self.device(), self.baud)
            .timeout(self.timeout)
            .open()
            .map(|port| Box::new(SerialIo(port)) as Box<dyn ControllerIo>)
            .map_err(io::Error::other)
    }
}

struct SerialIo(Box<dyn serialport::SerialPort>);

impl Read for SerialIo {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for SerialIo {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_configuration_is_owned_and_checked_without_opening_hardware() {
        let factory =
            SerialPortFactory::new("/dev/ttyUSB0", 115_200, Duration::from_millis(2)).unwrap();
        assert_eq!(factory.device(), "/dev/ttyUSB0");
        assert_eq!(factory.baud(), 115_200);
        assert_eq!(factory.timeout(), Duration::from_millis(2));
        assert_eq!(
            SerialPortFactory::new("", 115_200, Duration::from_millis(2)),
            Err(SerialFactoryError::EmptyDevice)
        );
    }
}
