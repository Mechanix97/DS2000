//! Serial port connection.
//!
//! Reading is done by a dedicated task awaiting the framed stream, not by polling it with a
//! timeout. `serial2-tokio` is async, so a task parked on `next()` costs nothing until bytes
//! actually arrive — where the previous implementation woke four times a second and blocked up
//! to 100 ms each time whether or not the device had said anything.

use super::error::SerialPortError;
use super::messages::device_info::DeviceInfoMessage;
use super::messages::hello::HelloMessage;
use super::serial_message::SerialMessageCodec;
use super::serial_message::{PROTOCOL_VERSION, SerialMessage};

use common::task_guard::AbortOnDrop;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serial2_tokio::SerialPort;
use serialport::{SerialPortInfo, SerialPortType};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Duration, timeout};
use tokio_util::codec::Framed;
use tracing::{debug, info, warn};

type PortFramed = Framed<SerialPort, SerialMessageCodec>;
type PortWriter = SplitSink<PortFramed, SerialMessage>;

/// How long the device has to answer the hello before the port is rejected.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(200);

/// USB vendor id the device enumerates with: Raspberry Pi's, which the Arduino-Pico core uses.
///
/// Only ports behind this vendor are opened. The product id is not checked because it differs per
/// board (the RP2350-Zero, the Pico and the Pico 2 each have their own), and the hello is what
/// finally tells a DS2000 apart from any other RP2040/RP2350 board.
pub const DS2000_USB_VID: u16 = 0x2E8A;

/// Sent before the handshake hello: a bare delimiter, which protocol 1 reads as an empty frame.
///
/// It ends whatever the device's receive buffer still holds, such as a frame cut short when the
/// application last closed the port, so the hello always starts a fresh frame instead of being
/// appended to a broken one and failing its CRC.
const HELLO_PREAMBLE: [u8; 1] = [0x00];

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
    device: Option<DeviceInfoMessage>,
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
                "Port {port_name:?} has a DS2000 on protocol {} (firmware {} on {}), this \
                 application speaks {PROTOCOL_VERSION}",
                device.protocol, device.firmware, device.board,
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
            "Serial port {port_name:?} connected, firmware {} on {}",
            device.firmware, device.board
        );
        Ok(())
    }

    /// Tries every port behind [`DS2000_USB_VID`] until one answers the handshake.
    ///
    /// Ports from any other vendor are never opened: opening one resets an Arduino, and the hello
    /// would be written to whatever else is listening (a printer, a modem, a Bluetooth link).
    ///
    /// When nothing usable is found but a DS2000 on the wrong protocol was, that is what gets
    /// reported: "no device" would send the user looking for a cable fault instead of an update.
    pub async fn auto_connect(
        &mut self,
        baudrate: u32,
        timeout: Duration,
    ) -> Result<(), SerialPortError> {
        let mut incompatible = None;
        for path in candidate_ports()? {
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
    pub fn device(&self) -> Option<DeviceInfoMessage> {
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
/// The USB vendor filter only narrows discovery down to RP2040/RP2350 boards; this is what tells a
/// DS2000 apart from any other one. Whether the reported protocol is one this application speaks
/// is for the caller to decide.
async fn handshake(framed: &mut PortFramed) -> Result<DeviceInfoMessage, SerialPortError> {
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
        Ok(Some(Ok(Ok(SerialMessage::DeviceInfo(info))))) => Ok(info),
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
        Err(_) => Err(SerialPortError::TimedOut),
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

/// Lists the serial ports that may have a DS2000 behind them, in a stable order.
///
/// Enumeration failing is reported rather than panicking: it happens on machines with unusual
/// driver setups, and it must not take the application down.
fn candidate_ports() -> Result<Vec<PathBuf>, SerialPortError> {
    let ports = serialport::available_ports().map_err(|err| {
        warn!("Could not enumerate serial ports: {err}");
        SerialPortError::PortNotAvailable
    })?;
    Ok(ds2000_ports(ports))
}

/// Keeps the USB ports whose vendor is [`DS2000_USB_VID`], sorted by name.
fn ds2000_ports(ports: Vec<SerialPortInfo>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = ports
        .into_iter()
        .filter(|port| {
            let matches = matches!(&port.port_type, SerialPortType::UsbPort(usb) if usb.vid == DS2000_USB_VID);
            if !matches {
                debug!("Skipping serial port {}: not a DS2000 USB device", port.port_name);
            }
            matches
        })
        .map(|port| PathBuf::from(port.port_name))
        .collect();
    candidates.sort();
    candidates
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

    fn usb_port(name: &str, vid: u16, pid: u16) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.to_owned(),
            port_type: SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: None,
                manufacturer: None,
                product: None,
            }),
        }
    }

    #[test]
    fn only_raspberry_pi_usb_ports_are_candidates() {
        let ports = vec![
            usb_port("COM9", 0x2341, 0x0043), // Arduino Uno
            SerialPortInfo {
                port_name: "COM1".to_owned(),
                port_type: SerialPortType::PciPort,
            },
            SerialPortInfo {
                port_name: "COM6".to_owned(),
                port_type: SerialPortType::BluetoothPort,
            },
            usb_port("COM4", DS2000_USB_VID, 0x10B0), // RP2350-Zero
            usb_port("COM3", 0x1A86, 0x7523),         // CH340 adapter
            usb_port("COM12", DS2000_USB_VID, 0x000A), // Pico
            SerialPortInfo {
                port_name: "COM2".to_owned(),
                port_type: SerialPortType::Unknown,
            },
        ];

        assert_eq!(
            ds2000_ports(ports),
            [PathBuf::from("COM12"), PathBuf::from("COM4")]
        );
    }

    #[test]
    fn no_matching_port_leaves_nothing_to_open() {
        assert!(ds2000_ports(vec![usb_port("COM9", 0x2341, 0x0043)]).is_empty());
        assert!(ds2000_ports(Vec::new()).is_empty());
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

    /// A leftover byte in the device's receive buffer (such as the start of a frame the previous
    /// connection never finished) must not stop the next handshake from connecting.
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

        // Leave an unterminated byte in the device's buffer.
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
