import { invoke, Channel } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-shell'
import { t, ready, onLanguageChange } from './i18n.js'

// About tab: check for a newer release and install it (#30). The backend does the work
// (controller/updater.rs); this only tracks what to show.

const statusLabel = document.getElementById('update-status');
const button = document.getElementById('update-button');
const progressBar = document.getElementById('update-progress');
const releaseLink = document.getElementById('update-release');
const prereleasesSwitch = document.getElementById('update-prereleases');

// idle | checking | upToDate | available | checkFailed | checkUnavailable | downloading |
// installing | installFailed
let state = 'idle';
let version = null;
let releaseUrl = null;
let downloaded = 0;
let total = null;

// Rendered from the state rather than set piecemeal, so a language change redraws it correctly.
function render() {
  const busy = state === 'checking' || state === 'downloading' || state === 'installing';
  const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;

  const status = {
    idle: '',
    checking: t('about.checking'),
    upToDate: t('about.upToDate'),
    available: t('about.available', { version }),
    checkFailed: t('about.checkFailed'),
    checkUnavailable: t('about.checkUnavailable'),
    downloading: percent === null
      ? t('about.downloadingNoSize')
      : t('about.downloading', { percent: `${percent}%` }),
    installing: t('about.installing'),
    installFailed: t('about.installFailed'),
  }[state];
  statusLabel.textContent = status;
  statusLabel.classList.toggle('ok', state === 'upToDate');
  statusLabel.classList.toggle('available', state === 'available');
  statusLabel.classList.toggle('error', ['checkFailed', 'checkUnavailable', 'installFailed'].includes(state));

  // Once an update is found the button installs it; everywhere else it checks again.
  const installs = state === 'available' || state === 'downloading' || state === 'installing';
  button.textContent = installs ? t('about.install', { version }) : t('about.checkUpdates');
  button.disabled = busy;

  releaseLink.hidden = !(installs && releaseUrl);

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
      // The manifest is not signed, so only a web page is opened from it.
      releaseUrl = update.releaseUrl?.startsWith('https://') ? update.releaseUrl : null;
      setState('available');
    } else {
      setState('upToDate');
    }
  } catch (error) {
    console.warn('Could not check for updates:', error);
    setState(error === 'unavailable' ? 'checkUnavailable' : 'checkFailed');
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

prereleasesSwitch.addEventListener('change', async () => {
  try {
    await invoke('set_update_prereleases', { enabled: prereleasesSwitch.checked });
  } catch (error) {
    console.error('Could not change the pre-release setting:', error);
    prereleasesSwitch.checked = !prereleasesSwitch.checked;
    return;
  }
  // An update found on the other channel no longer applies; the backend dropped it too.
  if (state === 'available' || state === 'upToDate') {
    setState('idle');
  }
});

releaseLink.addEventListener('click', async () => {
  await open(releaseUrl);
});

button.addEventListener('click', () => {
  if (state === 'available') {
    installUpdate();
  } else {
    checkForUpdates();
  }
});

onLanguageChange(render);
ready.then(render);
invoke('update_prereleases')
  .then(enabled => { prereleasesSwitch.checked = enabled; })
  .catch(error => console.error('Could not read the pre-release setting:', error));
