use crate::error::SerialPortError;
use crate::serial_state::DeviceStatus;
use crate::serial_state::SerialPortHandler;
use crate::serial_state::SerialPortProtocol;
use crate::serial_state::SerialPortState;
use crate::serial_state::SerialWorkerEvent;

use common::rgb_update::RGBConfig;

use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::debug;

pub struct SerialWorker {
    serial_port_handler: SerialPortHandler,
}

impl SerialWorker {
    /// Creates the worker. Frames and connection changes are announced on `observer` as they
    /// happen; nothing has to poll this worker.
    pub async fn new(
        baudrate: u32,
        timeout: Duration,
        observer: mpsc::UnboundedSender<SerialWorkerEvent>,
    ) -> Self {
        let serial_port_handler = SerialPortState::spawn(baudrate, timeout, observer).await;

        Self {
            serial_port_handler,
        }
    }

    /// Begins connecting, trying `last_used_port` first so the common case skips a full scan.
    pub async fn start(&mut self, last_used_port: Option<String>) -> Result<(), SerialPortError> {
        debug!("Serial worker started");
        self.serial_port_handler
            .connect(last_used_port)
            .map_err(SerialPortError::ActorError)
    }

    pub async fn get_port_name(&mut self) -> Result<Option<String>, SerialPortError> {
        self.serial_port_handler
            .port_name()
            .await
            .map_err(SerialPortError::ActorError)
    }

    pub async fn set_voice_settings(
        &mut self,
        mute: bool,
        deafen: bool,
    ) -> Result<(), SerialPortError> {
        self.serial_port_handler
            .set_voice_settings(mute, deafen)
            .map_err(SerialPortError::ActorError)
    }

    pub async fn is_connected(&mut self) -> Result<bool, SerialPortError> {
        self.serial_port_handler
            .serial_port_status()
            .await
            .map_err(SerialPortError::ActorError)
    }

    /// The device as last announced, with what it reported in the handshake.
    pub async fn device_status(&mut self) -> Result<DeviceStatus, SerialPortError> {
        self.serial_port_handler
            .device()
            .await
            .map_err(SerialPortError::ActorError)
    }

    /// Sends the device to USB boot for a firmware update and stops reconnecting until
    /// [`Self::resume`]. Returns whether there was a device to send.
    pub async fn enter_bootloader(&mut self) -> Result<bool, SerialPortError> {
        self.serial_port_handler
            .enter_bootloader()
            .await
            .map_err(SerialPortError::ActorError)
    }

    /// Reconnects again once a firmware update is done or abandoned.
    pub async fn resume(&mut self) -> Result<(), SerialPortError> {
        self.serial_port_handler
            .resume()
            .map_err(SerialPortError::ActorError)
    }

    pub async fn shutdown(&mut self) -> Result<(), SerialPortError> {
        debug!("Shutting down serial worker");
        self.serial_port_handler
            .shutdown()
            .await
            .map_err(SerialPortError::ActorError)
    }

    pub async fn set_rgb_config(&mut self, rgb_update: &RGBConfig) -> Result<(), SerialPortError> {
        self.serial_port_handler
            .rgb_update(rgb_update.clone())
            .map_err(SerialPortError::ActorError)
    }
}
