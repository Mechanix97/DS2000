import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

var mute = false;
var deaf = false;

const micButton = document.getElementById('mic-icon');
const headsetButton = document.getElementById('headset-icon');
const deviceFirmware = document.getElementById('device-firmware');

// The keys in the RGB tab's drawing of the device, by the name the backend reports them with.
const deviceKeys = Object.fromEntries(
  [...document.querySelectorAll('.device-key')].map(key => [key.dataset.key, key])
);
// How long a key stays lit after a press. The device reports presses only, not releases.
const KEY_FLASH_MS = 220;

// The preview's LEDs, which follow the voice state the way the firmware's do (main.cpp,
// ledsShow): the mute LED while muted or deafened, the deafen LED while deafened.
const previewMuteLed = document.getElementById('preview-led1');
const previewDeafenLed = document.getElementById('preview-led2');

function updateIcons() {
  micButton.setAttribute('aria-pressed', String(mute || deaf));
  headsetButton.setAttribute('aria-pressed', String(deaf));
  deviceKeys.mute.dataset.active = String(mute || deaf);
  deviceKeys.deafen.dataset.active = String(deaf);
  previewMuteLed.dataset.on = String(mute || deaf);
  previewDeafenLed.dataset.on = String(deaf);
}

listen('DEVICE_BUTTON_EVENT', event => {
  const key = deviceKeys[event.payload];
  if (!key) {
    console.error('Unexpected device button payload:', event.payload);
    return;
  }
  // Restart the flash if the same key is pressed again before it went out.
  clearTimeout(key.flashTimer);
  key.classList.add('pressed');
  key.flashTimer = setTimeout(() => key.classList.remove('pressed'), KEY_FLASH_MS);
});

micButton.addEventListener('click', () => {
  invoke('ds_set_voice_settings_command', { mute: !mute, deaf: deaf });
});

headsetButton.addEventListener('click', () => {
  invoke('ds_set_voice_settings_command', { mute: mute, deaf: !deaf });
});

// These events are serialized by serde, so the payload arrives as a real object or boolean.
// They are also only emitted when the value actually changes, so every one of them is news.
listen('DISCORD_VOICE_SETTINGS_EVENT', event => {
  const payload = event.payload;
  if (!payload || typeof payload !== 'object') {
    console.error('Unexpected voice settings payload:', payload);
    return;
  }
  mute = Boolean(payload.mute);
  deaf = Boolean(payload.deafen);
  updateIcons();
});

// An outdated device is worth identifying too: that is exactly when a bug report needs it.
listen('SERIAL_CONNECTION_STATUS_EVENT', event => {
  const payload = event.payload;
  deviceFirmware.textContent = payload?.firmware ? `${payload.firmware} (${payload.board})` : '—';
});

// A single source of truth for the version: the one Tauri was built with.
invoke('app_version').then(version => {
  const el = document.getElementById('app-version');
  if (el) el.textContent = version;
}).catch(error => console.error('Could not read the version:', error));

invoke('controller_start');
