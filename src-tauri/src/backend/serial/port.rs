//! Serial port connection.
//!
//! Reading is done by a dedicated task awaiting the framed stream, not by polling it with a
//! timeout. `serial2-tokio` is async, so a task parked on `next()` costs nothing until bytes
//! actually arrive — where the previous implementation woke four times a second and blocked up
//! to 100 ms each time whether or not the device had said anything.

use super::error::SerialPortError;
use super::messages::ping::PingMessage;
use super::messages::pong::DeviceVersion;
use super::serial_message::SerialMessageCodec;
use super::serial_message::{PROTOCOL_VERSION, SerialMessage};

use common::task_guard::AbortOnDrop;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serial2_tokio::SerialPort;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, timeout};
use tokio_util::codec::Framed;
use tracing::{debug, info, warn};

type PortFramed = Framed<SerialPort, SerialMessageCodec>;
type PortWriter = SplitSink<PortFramed, SerialMessage>;

/// How long the device has to answer the handshake ping before the port is rejected.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(200);

/// Something the reader task observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialEvent {
    Message(SerialMessage),
    /// The port closed or failed. The state machine reconnects with backoff.
    Disconnected,
}

#[derive(Clone)]
pub struct Port {
    pub name: Option<String>,
    writer: Option<Arc<Mutex<PortWriter>>>,
    _reader_task: Option<Arc<AbortOnDrop>>,
    events: mpsc::UnboundedSender<SerialEvent>,
    /// What the connected device reported in the handshake. `Some` exactly while connected.
    device: Option<DeviceVersion>,
}

impl Port {
    pub fn new(events: mpsc::UnboundedSender<SerialEvent>) -> Port {
        Port {
            name: None,
            writer: None,
            _reader_task: None,
            events,
            device: None,
        }
    }

    /// Opens a port, verifies a DS-2000 speaking this protocol is on the other end, and starts
    /// reading.
    ///
    /// The handshake runs on the whole framed stream before it is split, so the ping/pong
    /// exchange stays a simple request/response and the reader task only starts once the device
    /// has proven itself. A DS2000 on another protocol revision fails with
    /// [`SerialPortError::IncompatibleDevice`] and the port is released: frames exchanged with it
    /// would be misread in both directions.
    pub async fn connect_and_authenticate(
        &mut self,
        port_name: &Path,
        baudrate: u32,
        _timeout: Duration,
    ) -> Result<(), SerialPortError> {
        if self.is_connected() {
            return Err(SerialPortError::PortAlreadyConnected);
        }

        let port = SerialPort::open(port_name, baudrate).map_err(|err| {
            debug!("Port {port_name:?} is not available: {err}");
            SerialPortError::PortNotAvailable
        })?;
        port.set_dtr(true)?;
        port.set_rts(true)?;

        let mut framed = Framed::new(port, SerialMessageCodec);
        let device = handshake(&mut framed).await?;
        if device.protocol != PROTOCOL_VERSION {
            warn!(
                "Port {port_name:?} has a DS2000 on protocol {} (firmware {}), this application                  speaks {PROTOCOL_VERSION}",
                device.protocol,
                describe_firmware(&device),
            );
            return Err(SerialPortError::IncompatibleDevice(device));
        }

        let (writer, reader) = framed.split();
        let reader_task = tokio::spawn(read_loop(reader, self.events.clone()));

        self.writer = Some(Arc::new(Mutex::new(writer)));
        self._reader_task = Some(AbortOnDrop::new(reader_task));
        self.name = port_name.to_str().map(str::to_owned);
        self.device = Some(device);

        info!(
            "Serial port {port_name:?} connected, firmware {}",
            describe_firmware(&device)
        );
        Ok(())
    }

    /// Tries every available port until one answers the handshake.
    ///
    /// When nothing usable is found but a DS2000 on the wrong protocol was, that is what gets
    /// reported: "no device" would send the user looking for a cable fault instead of an update.
    pub async fn auto_connect(
        &mut self,
        baudrate: u32,
        timeout: Duration,
    ) -> Result<(), SerialPortError> {
        let mut available_ports = available_ports()?;
        available_ports.sort();

        let mut incompatible = None;
        for path in available_ports {
            debug!("Trying serial port {path:?}");
            match self
                .connect_and_authenticate(&path, baudrate, timeout)
                .await
            {
                Ok(()) => return Ok(()),
                Err(err @ SerialPortError::IncompatibleDevice(_)) => incompatible = Some(err),
                Err(_) => {}
            }
        }
        Err(incompatible.unwrap_or(SerialPortError::PortNotConnected))
    }

    pub async fn disconnect(&mut self) {
        if self.is_connected() {
            if let Some(name) = &self.name {
                info!("Serial port {name} disconnected");
            }
        }
        // Dropping the writer and the guard closes the port and stops the reader task.
        self.writer = None;
        self._reader_task = None;
        self.name = None;
        self.device = None;
    }

    pub fn is_connected(&self) -> bool {
        self.device.is_some()
    }

    /// What the connected device reported in the handshake, or `None` while disconnected.
    pub fn device(&self) -> Option<DeviceVersion> {
        self.device
    }

    pub async fn send_message(&self, message: &SerialMessage) -> Result<(), SerialPortError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(SerialPortError::PortNotConnected)?;
        writer.lock().await.send(message.clone()).await
    }
}

/// Confirms a DS-2000 is on the other end by exchanging ping/pong, and returns what it reported.
///
/// Without it, `auto_connect` would happily latch onto any serial device on the machine — a
/// printer, an Arduino, a Bluetooth adapter. Whether the reported protocol is one this application
/// speaks is for the caller to decide.
async fn handshake(framed: &mut PortFramed) -> Result<DeviceVersion, SerialPortError> {
    framed
        .send(SerialMessage::Ping(PingMessage {}))
        .await
        .map_err(|err| {
            debug!("Could not send the handshake ping: {err}");
            SerialPortError::AuthenticationFailed
        })?;

    match timeout(HANDSHAKE_TIMEOUT, framed.next()).await {
        Ok(Some(Ok(SerialMessage::Pong(pong)))) => Ok(pong.device),
        Ok(Some(Ok(other))) => {
            debug!("Handshake answered with {other:?} instead of a pong");
            Err(SerialPortError::AuthenticationFailed)
        }
        Ok(Some(Err(err))) => {
            debug!("Handshake reply could not be decoded: {err}");
            Err(SerialPortError::AuthenticationFailed)
        }
        Ok(None) => Err(SerialPortError::PortNotConnected),
        Err(_) => Err(SerialPortError::TimedOut),
    }
}

/// The firmware version for a log line. Firmware before versioning has none to report.
fn describe_firmware(device: &DeviceVersion) -> String {
    device
        .firmware
        .map_or_else(|| "unversioned".to_owned(), |version| version.to_string())
}

/// Awaits frames from the device for as long as the port is open.
///
/// A malformed frame is logged and skipped rather than treated as a disconnection: line noise
/// should cost one dropped message, not a reconnect cycle. Only an I/O failure ends the loop.
async fn read_loop(
    mut reader: SplitStream<PortFramed>,
    events: mpsc::UnboundedSender<SerialEvent>,
) {
    while let Some(frame) = reader.next().await {
        match frame {
            Ok(message) => {
                debug!("Serial message received: {message:?}");
                if events.send(SerialEvent::Message(message)).is_err() {
                    // Nobody is listening any more, so the connection is being torn down.
                    return;
                }
            }
            Err(SerialPortError::IoError(err)) => {
                debug!("Serial port I/O error, dropping the connection: {err}");
                break;
            }
            Err(err) => warn!("Discarding a malformed serial frame: {err}"),
        }
    }

    let _ = events.send(SerialEvent::Disconnected);
}

/// Lists candidate serial ports.
///
/// Enumeration failing is reported rather than panicking: it happens on machines with unusual
/// driver setups, and it must not take the application down.
fn available_ports() -> Result<Vec<PathBuf>, SerialPortError> {
    SerialPort::available_ports().map_err(|err| {
        warn!("Could not enumerate serial ports: {err}");
        SerialPortError::PortNotAvailable
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that open the real device.
    ///
    /// A serial port admits a single handle, and the test harness runs tests in parallel by
    /// default, so without this the second test to start finds the port already taken and fails
    /// with `PortNotConnected` — a failure about the harness, not about the device.
    ///
    /// Tokio's mutex rather than the standard one: the guard is held across the whole test body,
    /// awaits included. It also does not poison, so one failing hardware test releases the device
    /// instead of cascading into every later one.
    static DEVICE: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn a_fresh_port_is_not_connected_and_refuses_to_send() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let port = Port::new(tx);

        assert!(!port.is_connected());
        assert_eq!(
            port.send_message(&SerialMessage::Ping(PingMessage {}))
                .await,
            Err(SerialPortError::PortNotConnected)
        );
    }

    #[tokio::test]
    async fn disconnecting_an_unconnected_port_is_harmless() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut port = Port::new(tx);

        port.disconnect().await;

        assert!(!port.is_connected());
        assert!(port.name.is_none());
    }

    #[tokio::test]
    #[ignore = "needs a DS-2000 device connected over USB; run with --ignored"]
    async fn autoconnect_finds_the_device_and_receives_its_frames() {
        let _device = DEVICE.lock().await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut port = Port::new(tx);

        port.auto_connect(115200, Duration::from_millis(1000))
            .await
            .expect("a DS-2000 should be connected");
        assert!(port.is_connected());

        // The device answers a ping, which proves the reader task is delivering frames.
        port.send_message(&SerialMessage::Ping(PingMessage {}))
            .await
            .expect("sends");

        let event = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a frame should arrive")
            .expect("the channel stays open");

        assert!(matches!(
            event,
            SerialEvent::Message(SerialMessage::Pong(_))
        ));

        port.disconnect().await;
        assert!(!port.is_connected());
    }

    /// Reconnecting has to work repeatedly: the reader task and the port handle from the previous
    /// connection must be gone, or the port stays locked and the second attempt fails.
    #[tokio::test]
    #[ignore = "needs a DS-2000 device connected over USB; run with --ignored"]
    async fn the_port_can_be_reopened_after_disconnecting() {
        let _device = DEVICE.lock().await;

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut port = Port::new(tx);

        for attempt in 1..=3 {
            port.auto_connect(115200, Duration::from_millis(1000))
                .await
                .unwrap_or_else(|err| panic!("attempt {attempt} should connect: {err}"));
            assert!(port.is_connected(), "attempt {attempt}");

            port.disconnect().await;
            assert!(!port.is_connected(), "attempt {attempt}");

            // Give the OS a moment to release the handle before reopening.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
