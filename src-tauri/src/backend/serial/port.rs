//! Serial port connection.
//!
//! Reading is done by a dedicated task awaiting the framed stream, not by polling it with a
//! timeout. `serial2-tokio` is async, so a task parked on `next()` costs nothing until bytes
//! actually arrive — where the previous implementation woke four times a second and blocked up
//! to 100 ms each time whether or not the device had said anything.

use super::error::SerialPortError;
use super::messages::device_info::DeviceIdentity;
use super::messages::hello::HelloMessage;
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

/// How long the device has to answer the hello, and separately the legacy probe, before the port
/// is rejected.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(200);

/// Asks firmware from before protocol 1 to identify itself, in its own framing.
///
/// That firmware ends frames on a bare `0xFF` and answers `00 FF` (its ping) with `01 FF` (its
/// pong). The leading `0xFF` flushes whatever the hello left in its buffer first; the COBS hello
/// always starts with `0x01`, which that firmware reads as a pong and ignores, so nothing it acts
/// on is ever sent.
const LEGACY_PROBE: [u8; 3] = [0xFF, 0x00, 0xFF];

/// Sent before the handshake hello: a bare delimiter, which protocol 1 reads as an empty frame.
///
/// It ends whatever the device's receive buffer still holds. Without it, the trailing `0xFF` of a
/// legacy probe sent to protocol-1 firmware (which happens whenever a hello goes unanswered) stayed
/// in that buffer, got prepended to the next hello and broke it, which triggered another probe:
/// the application never connected again until the device was unplugged.
const HELLO_PREAMBLE: [u8; 1] = [0x00];
const LEGACY_REPLY: [u8; 2] = [0x01, 0xFF];

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
    device: Option<DeviceIdentity>,
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
    /// The handshake runs on the whole framed stream before it is split, so the hello / device info
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
    pub fn device(&self) -> Option<DeviceIdentity> {
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

/// Confirms a DS-2000 is on the other end by exchanging hello / device info, and returns what it
/// reported.
///
/// Without it, `auto_connect` would happily latch onto any serial device on the machine — a
/// printer, an Arduino, a Bluetooth adapter. Whether the reported protocol is one this application
/// speaks is for the caller to decide.
///
/// A device that does not answer the hello gets one more chance in the pre-COBS framing, so that
/// firmware from before protocol 1 is reported as out of date instead of being indistinguishable
/// from no device at all.
async fn handshake(framed: &mut PortFramed) -> Result<DeviceIdentity, SerialPortError> {
    framed
        .get_ref()
        .write_all(&HELLO_PREAMBLE)
        .await
        .map_err(|err| {
            debug!("Could not send the handshake preamble: {err}");
            SerialPortError::AuthenticationFailed
        })?;
    framed
        .send(SerialMessage::Hello(HelloMessage {}))
        .await
        .map_err(|err| {
            debug!("Could not send the handshake hello: {err}");
            SerialPortError::AuthenticationFailed
        })?;

    match timeout(HANDSHAKE_TIMEOUT, framed.next()).await {
        Ok(Some(Ok(Ok(SerialMessage::DeviceInfo(info))))) => Ok(info.identity()),
        Ok(Some(Ok(Ok(other)))) => {
            debug!("Handshake answered with {other:?} instead of device info");
            Err(SerialPortError::AuthenticationFailed)
        }
        Ok(Some(Ok(Err(err)))) => {
            debug!("Handshake reply could not be decoded: {err}");
            Err(SerialPortError::AuthenticationFailed)
        }
        Ok(Some(Err(err))) => Err(err),
        Ok(None) => Err(SerialPortError::PortNotConnected),
        Err(_) if probe_legacy_firmware(framed).await => Ok(DeviceIdentity::unversioned()),
        Err(_) => Err(SerialPortError::TimedOut),
    }
}

/// Whether the port answers [`LEGACY_PROBE`] the way pre-protocol-1 firmware does.
///
/// The reply contains no `0x00`, so the codec never completes a frame from it: it stays in the
/// read buffer, which is where it is looked for once the wait is over.
async fn probe_legacy_firmware(framed: &mut PortFramed) -> bool {
    framed.read_buffer_mut().clear();
    if let Err(err) = framed.get_ref().write_all(&LEGACY_PROBE).await {
        debug!("Could not send the legacy probe: {err}");
        return false;
    }

    // Only the timeout can end this wait usefully; a frame decoded here would be a new-framing
    // reply, which the hello already had its chance to receive.
    let _ = timeout(HANDSHAKE_TIMEOUT, framed.next()).await;
    framed
        .read_buffer()
        .windows(LEGACY_REPLY.len())
        .any(|window| window == LEGACY_REPLY)
}

/// The firmware version and board for a log line. Firmware before versioning reports neither.
fn describe_firmware(device: &DeviceIdentity) -> String {
    match (device.firmware, device.board) {
        (Some(version), Some(board)) => format!("{version} on {board}"),
        (Some(version), None) => version.to_string(),
        _ => "unversioned".to_owned(),
    }
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
            Ok(Ok(message)) => {
                debug!("Serial message received: {message:?}");
                if events.send(SerialEvent::Message(message)).is_err() {
                    // Nobody is listening any more, so the connection is being torn down.
                    return;
                }
            }
            Ok(Err(err)) => warn!("Discarding a malformed serial frame: {err}"),
            Err(err) => {
                debug!("Serial port error, dropping the connection: {err}");
                break;
            }
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
            port.send_message(&SerialMessage::Hello(HelloMessage {}))
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

        // The device answers a hello, which proves the reader task is delivering frames.
        port.send_message(&SerialMessage::Hello(HelloMessage {}))
            .await
            .expect("sends");

        let event = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a frame should arrive")
            .expect("the channel stays open");

        assert!(matches!(
            event,
            SerialEvent::Message(SerialMessage::DeviceInfo(_))
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

    /// A leftover byte in the device's receive buffer (such as the tail of a legacy probe sent
    /// after a missed hello) must not stop the next handshake from connecting.
    #[tokio::test]
    #[ignore = "needs a DS-2000 device connected over USB; run with --ignored"]
    async fn a_stray_byte_in_the_device_buffer_does_not_block_the_handshake() {
        let _device = DEVICE.lock().await;

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut port = Port::new(tx);
        port.auto_connect(115200, Duration::from_millis(1000))
            .await
            .expect("a DS-2000 should be connected");
        let name = port.name.clone().expect("a connected port has a name");
        port.disconnect().await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Leave the tail of a legacy probe in the device's buffer, unterminated.
        let raw = SerialPort::open(&name, 115200).expect("the port reopens");
        raw.set_dtr(true).expect("sets DTR");
        raw.write_all(&[0xFF]).await.expect("writes");
        drop(raw);
        tokio::time::sleep(Duration::from_millis(200)).await;

        for attempt in 1..=3 {
            port.auto_connect(115200, Duration::from_millis(1000))
                .await
                .unwrap_or_else(|err| panic!("attempt {attempt} should connect: {err}"));
            port.disconnect().await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}
