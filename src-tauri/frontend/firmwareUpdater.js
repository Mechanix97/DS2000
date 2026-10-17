import { invoke, Channel } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { t, ready, onLanguageChange } from './i18n.js'

// About tab: check for newer device firmware and install it (#35). The backend does the work
// (controller/firmware.rs); this only tracks what to show.

const statusLabel = document.getElementById('firmware-status');
const button = document.getElementById('firmware-button');

// idle | checking | upToDate | available | noDevice | unknownBoard | checkFailed | checkUnavailable
// | downloading | rebooting | flashing | restarting | done | installFailed
let state = 'idle';
let version = null;

const BUSY = ['checking', 'downloading', 'rebooting', 'flashing', 'restarting'];
const INSTALLING = ['available', 'downloading', 'rebooting', 'flashing', 'restarting'];
const ERRORS = ['noDevice', 'unknownBoard', 'checkFailed', 'checkUnavailable', 'installFailed'];
// What firmware_check rejects with, mapped to the state that explains it.
const CHECK_ERRORS = {
  noDevice: 'noDevice',
  unknownBoard: 'unknownBoard',
  offline: 'checkFailed',
  unavailable: 'checkUnavailable',
};

// Rendered from the state rather than set piecemeal, so a language change redraws it correctly.
function render() {
  const busy = BUSY.includes(state);
  const keys = {
    idle: null,
    checking: 'firmwareUpdate.checking',
    upToDate: 'firmwareUpdate.upToDate',
    available: 'firmwareUpdate.available',
    noDevice: 'firmwareUpdate.noDevice',
    unknownBoard: 'firmwareUpdate.unknownBoard',
    checkFailed: 'firmwareUpdate.checkFailed',
    checkUnavailable: 'firmwareUpdate.checkUnavailable',
    downloading: 'firmwareUpdate.downloading',
    rebooting: 'firmwareUpdate.rebooting',
    flashing: 'firmwareUpdate.flashing',
    restarting: 'firmwareUpdate.restarting',
    done: 'firmwareUpdate.done',
    installFailed: 'firmwareUpdate.installFailed',
  };
  statusLabel.textContent = keys[state] ? t(keys[state], { version }) : '';
  statusLabel.classList.toggle('ok', state === 'upToDate' || state === 'done');
  statusLabel.classList.toggle('available', state === 'available');
  statusLabel.classList.toggle('error', ERRORS.includes(state));

  // Once an update is found the button installs it; everywhere else it checks again.
  button.textContent = INSTALLING.includes(state)
    ? t('firmwareUpdate.install', { version })
    : t('firmwareUpdate.check');
  button.disabled = busy;

  // The device is offline while it updates, so its controls would do nothing.
  document.body.classList.toggle('firmware-updating', busy && state !== 'checking');
}

function setState(next) {
  state = next;
  render();
}

async function checkForUpdates() {
  setState('checking');
  try {
    const update = await invoke('firmware_check');
    if (update) {
      version = update.version;
      setState('available');
    } else {
      setState('upToDate');
    }
  } catch (error) {
    console.warn('Could not check for firmware updates:', error);
    setState(CHECK_ERRORS[error] ?? 'checkFailed');
  }
}

async function installUpdate() {
  setState('downloading');

  const onProgress = new Channel();
  onProgress.onmessage = message => {
    if (BUSY.includes(message.event)) {
      setState(message.event);
    }
  };

  try {
    version = await invoke('firmware_install', { onProgress });
    setState('done');
  } catch (error) {
    console.error('Could not update the firmware:', error);
    // The backend dropped the release it had found, so the next click checks again.
    setState('installFailed');
  }
}

button.addEventListener('click', () => {
  if (state === 'available') {
    installUpdate();
  } else {
    checkForUpdates();
  }
});

// A device plugged in after "connect the device" was shown deserves a fresh look, and a device on
// an outdated protocol is exactly the one that needs this, so both are checked without a click.
listen('SERIAL_CONNECTION_STATUS_EVENT', event => {
  const deviceState = event.payload?.state;
  const attached = deviceState === 'connected' || deviceState === 'firmwareOutdated';
  if (attached && (state === 'noDevice' || (state === 'idle' && deviceState === 'firmwareOutdated'))) {
    checkForUpdates();
  }
});

onLanguageChange(render);
ready.then(render);
