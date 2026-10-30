import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { t, ready, onLanguageChange } from './i18n.js'
import { chosenDevices, deviceRows } from './deviceChoices.js'

// Settings → Audio devices: which of Discord's inputs and outputs Shift + mute and Shift + deafen
// switch between.

const status = document.getElementById('voice-devices-status');
const feedback = document.getElementById('voice-devices-feedback');
const lists = {
  input: document.getElementById('voice-devices-input'),
  output: document.getElementById('voice-devices-output'),
};

let choices = null;
// Only the newest load renders: a slow answer must not overwrite a newer one.
let loadCount = 0;

function renderList(kind) {
  const list = lists[kind];
  const rows = deviceRows(choices.connected ? choices[kind].available : [], choices[kind].chosen);
  list.replaceChildren(...rows.map(row => {
    const label = document.createElement('label');
    label.className = 'setting-row';

    const text = document.createElement('span');
    text.className = 'setting-text';
    const name = document.createElement('span');
    name.className = 'setting-name';
    name.textContent = row.name || row.id;
    text.append(name);
    if (row.unplugged) {
      const hint = document.createElement('span');
      hint.className = 'setting-hint';
      hint.textContent = t('settings.devicesUnplugged');
      text.append(hint);
    }

    const toggle = document.createElement('input');
    toggle.type = 'checkbox';
    toggle.className = 'switch';
    toggle.checked = row.checked;
    toggle.addEventListener('change', () => {
      row.checked = toggle.checked;
      save(kind, rows);
    });

    label.append(text, toggle);
    return label;
  }));

  if (rows.length === 0) {
    const empty = document.createElement('p');
    empty.className = 'setting-hint';
    empty.textContent = t(choices.connected ? 'settings.devicesNone' : 'settings.devicesOffline');
    list.append(empty);
  }
}

function render() {
  if (!choices) {
    return;
  }
  status.hidden = choices.shiftDetectable;
  status.textContent = choices.shiftDetectable ? '' : t('settings.devicesUnsupported');
  renderList('input');
  renderList('output');
}

async function save(kind, rows) {
  const chosen = chosenDevices(rows);
  try {
    await invoke('set_voice_device_choices', { kind, chosen });
    choices[kind].chosen = chosen;
    feedback.textContent = '';
    feedback.classList.remove('error');
  } catch (error) {
    feedback.textContent = t('settings.devicesSaveFailed');
    feedback.classList.add('error');
    console.error('set_voice_device_choices failed:', error);
  }
  render();
}

async function load() {
  const current = ++loadCount;
  try {
    const loaded = await invoke('voice_device_choices');
    if (current === loadCount) {
      choices = loaded;
      render();
    }
  } catch (error) {
    console.error('voice_device_choices failed:', error);
  }
}

// Devices come and go with USB, so the lists are read again whenever the tab is opened, and once
// Discord connects.
document.querySelector('.section[data-tab="configuraciones"]').addEventListener('click', load);
listen('DISCORD_CONNECTION_STATUS_EVENT', event => {
  if (event.payload) {
    load();
  }
});
onLanguageChange(render);

ready.then(load);
