import { invoke, Channel } from '@tauri-apps/api/core'
import { t, ready, onLanguageChange } from './i18n.js'

// About tab: check for a newer release and install it (#30). The backend does the work
// (controller/updater.rs); this only tracks what to show.

const statusLabel = document.getElementById('update-status');
const button = document.getElementById('update-button');
const progressBar = document.getElementById('update-progress');

// idle | checking | upToDate | available | checkFailed | downloading | installing | installFailed
let state = 'idle';
let version = null;
let downloaded = 0;
let total = null;

function format(key, values = {}) {
  return Object.entries(values).reduce((text, [name, value]) => text.replace(`{${name}}`, value), t(key));
}

// Rendered from the state rather than set piecemeal, so a language change redraws it correctly.
function render() {
  const busy = state === 'checking' || state === 'downloading' || state === 'installing';
  const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;

  const status = {
    idle: '',
    checking: t('about.checking'),
    upToDate: t('about.upToDate'),
    available: format('about.available', { version }),
    checkFailed: t('about.checkFailed'),
    downloading: percent === null
      ? t('about.downloadingNoSize')
      : format('about.downloading', { percent: `${percent}%` }),
    installing: t('about.installing'),
    installFailed: t('about.installFailed'),
  }[state];
  statusLabel.textContent = status;
  statusLabel.classList.toggle('ok', state === 'upToDate');
  statusLabel.classList.toggle('available', state === 'available');
  statusLabel.classList.toggle('error', state === 'checkFailed' || state === 'installFailed');

  // Once an update is found the button installs it; everywhere else it checks again.
  const installs = state === 'available' || state === 'downloading' || state === 'installing';
  button.textContent = installs ? format('about.install', { version }) : t('about.checkUpdates');
  button.disabled = busy;

  progressBar.hidden = !(state === 'downloading' || state === 'installing');
  if (state === 'installing') {
    progressBar.value = 100;
  } else if (percent === null) {
    progressBar.removeAttribute('value');   // indeterminate
  } else {
    progressBar.value = percent;
  }
}

function setState(next) {
  state = next;
  render();
}

async function checkForUpdates() {
  setState('checking');
  try {
    const update = await invoke('update_check');
    if (update) {
      version = update.version;
      setState('available');
    } else {
      setState('upToDate');
    }
  } catch (error) {
    console.warn('Could not check for updates:', error);
    setState('checkFailed');
  }
}

async function installUpdate() {
  downloaded = 0;
  total = null;
  setState('downloading');

  const onProgress = new Channel();
  onProgress.onmessage = message => {
    if (message.event === 'started') {
      total = message.data.total;
    } else if (message.event === 'progress') {
      downloaded = message.data.downloaded;
    } else if (message.event === 'installing') {
      setState('installing');
      return;
    }
    render();
  };

  try {
    // Does not resolve on success: the app exits and the installer takes over.
    await invoke('update_install', { onProgress });
  } catch (error) {
    console.error('Could not install the update:', error);
    // The backend dropped the update it had found, so the next click checks again.
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

onLanguageChange(render);
ready.then(render);
