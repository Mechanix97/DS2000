//! Connection state machine for the serial device.
//!
//! Nothing runs on a timer while the port is open: frames arrive from the reader task and are
//! handled as they come. The only scheduled work is the reconnect attempt, which backs off so a
//! machine with no device attached is not rescanning its ports several times a second.

use common::rgb_update::RGBConfig;
use std::path::PathBuf;

use crate::messages::pong::DeviceVersion;
use crate::messages::rgb::RGBConfigMessage;
use crate::messages::voice_settings::VoiceSettingsMessage;

use crate::port::{Port, SerialEvent};
use crate::{error::SerialPortError, serial_message::SerialMessage};

use spawned_concurrency::tasks::{
    CallResponse, CastResponse, GenServer, GenServerHandle, send_after,
};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, info, warn};

/// First delay after a failed connection attempt.
const RECONNECT_BACKOFF_MIN: Duration = Duration::from_secs(1);
/// Ceiling for the backoff. Scanning every port has a cost, so an absent device settles here.
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(15);

pub type SerialPortHandler = GenServerHandle<SerialPortState>;

#[derive(Clone)]
pub enum InCallMessage {
    PortName,
    Shutdown,
    SerialPortStatus,
}

#[derive(Clone)]
pub enum InMessage {
    /// Attempts to bring the port up. Rescheduled with backoff only while it fails.
    Connect(Option<String>),
    SetVoiceSettings(bool, bool),
    RGBUpdate(RGBConfig),
    /// Something the reader task observed.
    Serial(SerialEvent),
}

#[derive(Clone, PartialEq)]
pub enum OutMessage {
    Done,
    PortName(Option<String>),
    SerialPortStatus(bool),
}

/// Whether a usable device is attached, as far as the application can tell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DeviceStatus {
    #[default]
    Disconnected,
    Connected(DeviceVersion),
    /// A DS2000 answered but speaks another protocol revision, so nothing is exchanged with it.
    /// Reported separately so the user is told to update rather than left seeing "not connected".
    Incompatible(DeviceVersion),
}

/// Something worth telling the controller about.
///
/// Emitted as it happens rather than queued for a poller, so a button press reaches Discord
/// immediately instead of waiting up to 100 ms for the next drain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SerialWorkerEvent {
    Message(SerialMessage),
    StatusChanged(DeviceStatus),
}

#[derive(Clone)]
pub struct SerialPortState {
    port: Port,
    baudrate: u32,
    timeout: Duration,
    /// Where frames and connection changes are announced. The controller listens here.
    observer: mpsc::UnboundedSender<SerialWorkerEvent>,
    /// Last status announced, so `StatusChanged` really means changed.
    announced: DeviceStatus,
    reconnect_backoff: Duration,
    /// Guards against several reconnect timers piling up, which would defeat the backoff.
    reconnect_scheduled: bool,
    shutdown: bool,
}

impl SerialPortState {
    pub async fn spawn(
        baudrate: u32,
        timeout: Duration,
        observer: mpsc::UnboundedSender<SerialWorkerEvent>,
    ) -> SerialPortHandler {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();

        let state = Self {
            port: Port::new(events_tx),
            baudrate,
            timeout,
            observer,
            announced: DeviceStatus::Disconnected,
            reconnect_backoff: RECONNECT_BACKOFF_MIN,
            reconnect_scheduled: false,
            shutdown: false,
        };
        let handle = state.start();

        // Bridges the reader task's frames into the actor's mailbox, so the actor only ever
        // reacts to messages and never polls the port.
        let mut forward_to = handle.clone();
        tokio::spawn(async move {
            while let Some(event) = events_rx.recv().await {
                if forward_to.cast(InMessage::Serial(event)).await.is_err() {
                    break;
                }
            }
        });

        handle
    }

    /// Schedules another connection attempt, doubling the delay up to the ceiling.
    fn schedule_reconnect(&mut self, handle: &GenServerHandle<Self>) {
        if self.shutdown || self.reconnect_scheduled {
            return;
        }
        self.reconnect_scheduled = true;
        send_after(
            self.reconnect_backoff,
            handle.clone(),
            InMessage::Connect(None),
        );
        self.reconnect_backoff = (self.reconnect_backoff * 2).min(RECONNECT_BACKOFF_MAX);
    }

    /// Announces a status transition, once per transition.
    fn announce(&mut self, status: DeviceStatus) {
        if status == self.announced {
            return;
        }
        self.announced = status;
        let _ = self.observer.send(SerialWorkerEvent::StatusChanged(status));
    }

    /// Announces whatever the port currently reports.
    fn announce_port_status(&mut self) {
        let status = self
            .port
            .device()
            .map_or(DeviceStatus::Disconnected, DeviceStatus::Connected);
        self.announce(status);
    }
}

impl GenServer for SerialPortState {
    type CallMsg = InCallMessage;
    type CastMsg = InMessage;
    type OutMsg = OutMessage;
    type Error = SerialPortError;

    async fn handle_cast(
        mut self,
        message: Self::CastMsg,
        handle: &GenServerHandle<Self>,
    ) -> CastResponse<Self> {
        if self.shutdown {
            return CastResponse::NoReply(self);
        }

        match message {
            InMessage::Connect(preferred_port) => {
                self.reconnect_scheduled = false;

                if self.port.is_connected() {
                    return CastResponse::NoReply(self);
                }

                // The port used last is tried first, which skips scanning every other device on
                // the machine in the common case.
                let connected = match &preferred_port {
                    Some(name) => {
                        debug!("Trying the last used port {name}");
                        self.port
                            .connect_and_authenticate(
                                &PathBuf::from(name),
                                self.baudrate,
                                self.timeout,
                            )
                            .await
                            .is_ok()
                    }
                    None => false,
                };

                if !connected
                    && let Err(err) = self.port.auto_connect(self.baudrate, self.timeout).await
                {
                    // An incompatible device keeps being retried like an absent one, so flashing
                    // new firmware is picked up without restarting the application.
                    match err {
                        SerialPortError::IncompatibleDevice(device) => {
                            self.announce(DeviceStatus::Incompatible(device));
                        }
                        err => {
                            debug!("No DS-2000 found on any serial port: {err}");
                            self.announce(DeviceStatus::Disconnected);
                        }
                    }
                    self.schedule_reconnect(handle);
                    return CastResponse::NoReply(self);
                }

                self.reconnect_backoff = RECONNECT_BACKOFF_MIN;
                self.announce_port_status();
                CastResponse::NoReply(self)
            }

            InMessage::Serial(SerialEvent::Message(message)) => {
                // Straight to the controller: no queue, no waiting for someone to drain it.
                let _ = self.observer.send(SerialWorkerEvent::Message(message));
                CastResponse::NoReply(self)
            }

            InMessage::Serial(SerialEvent::Disconnected) => {
                info!("Serial device disconnected, will try to reconnect");
                self.port.disconnect().await;
                self.announce_port_status();
                self.schedule_reconnect(handle);
                CastResponse::NoReply(self)
            }

            InMessage::SetVoiceSettings(mute, deafen) => {
                if self.port.is_connected()
                    && let Err(err) = self
                        .port
                        .send_message(&SerialMessage::VoiceSettings(VoiceSettingsMessage {
                            mute,
                            deafen,
                        }))
                        .await
                {
                    warn!("Could not send voice settings to the device: {err}");
                }
                CastResponse::NoReply(self)
            }

            InMessage::RGBUpdate(update) => {
                if self.port.is_connected()
                    && let Err(err) = self
                        .port
                        .send_message(&SerialMessage::RGBUpdate(RGBConfigMessage { update }))
                        .await
                {
                    warn!("Could not send the RGB update to the device: {err}");
                }
                CastResponse::NoReply(self)
            }
        }
    }

    async fn handle_call(
        mut self,
        message: Self::CallMsg,
        _handle: &GenServerHandle<Self>,
    ) -> CallResponse<Self> {
        match message {
            InCallMessage::PortName => {
                let name = self.port.name.clone();
                CallResponse::Reply(self, OutMessage::PortName(name))
            }
            InCallMessage::Shutdown => {
                self.shutdown = true;
                self.port.disconnect().await;
                CallResponse::Reply(self, OutMessage::Done)
            }
            InCallMessage::SerialPortStatus => {
                let connected = self.port.is_connected();
                CallResponse::Reply(self, OutMessage::SerialPortStatus(connected))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> (SerialPortState, mpsc::UnboundedReceiver<SerialWorkerEvent>) {
        let (port_tx, _port_rx) = mpsc::unbounded_channel();
        let (observer, observed) = mpsc::unbounded_channel();
        (
            SerialPortState {
                port: Port::new(port_tx),
                baudrate: 115200,
                timeout: Duration::from_millis(1000),
                observer,
                announced: DeviceStatus::Disconnected,
                reconnect_backoff: RECONNECT_BACKOFF_MIN,
                reconnect_scheduled: false,
                shutdown: false,
            },
            observed,
        )
    }

    fn outdated_device() -> DeviceVersion {
        DeviceVersion {
            protocol: crate::messages::pong::UNVERSIONED_PROTOCOL,
            firmware: None,
        }
    }

    #[test]
    fn a_status_change_is_announced_once_per_transition() {
        let (mut state, mut observed) = state();

        // Still disconnected, so there is nothing to announce.
        state.announce_port_status();
        assert!(observed.try_recv().is_err());

        state.announce(DeviceStatus::Incompatible(outdated_device()));
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            SerialWorkerEvent::StatusChanged(DeviceStatus::Incompatible(outdated_device()))
        );
        // Every retry against the same outdated device reports it again; only the first is news.
        state.announce(DeviceStatus::Incompatible(outdated_device()));
        assert!(observed.try_recv().is_err());

        // Unplugging it clears the warning.
        state.announce_port_status();
        assert_eq!(
            observed.try_recv().expect("an announcement"),
            SerialWorkerEvent::StatusChanged(DeviceStatus::Disconnected)
        );
        state.announce_port_status();
        assert!(observed.try_recv().is_err());
    }

    #[test]
    fn the_reconnect_backoff_grows_and_then_settles_at_the_ceiling() {
        let (mut state, _observed) = state();
        let mut delays = Vec::new();

        for _ in 0..8 {
            delays.push(state.reconnect_backoff);
            // Mirrors what schedule_reconnect does, without needing a live actor handle.
            state.reconnect_backoff = (state.reconnect_backoff * 2).min(RECONNECT_BACKOFF_MAX);
        }

        assert_eq!(delays[0], RECONNECT_BACKOFF_MIN);
        assert!(delays[1] > delays[0]);
        assert_eq!(*delays.last().expect("non-empty"), RECONNECT_BACKOFF_MAX);
    }
}
