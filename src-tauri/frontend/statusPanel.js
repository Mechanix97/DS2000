import { listen } from '@tauri-apps/api/event'
import { t, onLanguageChange } from './i18n.js'
import { selectTab } from './tabs.js'
import { showNotice, hideNotice, noticeKind } from './notices.js'

// The bottom of the sidebar: the Discord account, and above it notifications about the device,
// Discord and updates, each with a way to the place that fixes it. It only reads state; the tabs
// own the actions.

const userCard = document.getElementById('user-card');
const userName = document.getElementById('user-name');
const userAvatar = document.getElementById('user-avatar');
// What the avatar shows when there is no account picture to show.
const LOGO = userAvatar.getAttribute('src');
const userStatus = document.getElementById('user-status');
const discordBadge = document.getElementById('discord-badge');
const aboutBadge = document.getElementById('about-badge');

/// Problems found right at start are held back this long: the device and Discord usually connect
/// within it, and a red notice that turns green a second later is just noise.
const STARTUP_GRACE_MS = 3000;
let starting = true;

// Mirrors the backend's SerialStatusPayload: `state` is one of connected, disconnected,
// firmwareOutdated or appOutdated; `firmware` and `board` are what the device reported, or null
// while nothing is attached. Null until the backend first reports it.
let serial = null;
let discordConnected = false;
// Whether Discord itself is open. Unknown counts as open, so nothing claims it is closed before the
// backend has looked.
let discordRunning = true;
// Whether credentials are stored, or null until the Discord tab has asked. Without them there is
// nothing to wait for, so the notice says "not set up" rather than "not connected".
let hasCredentials = null;
// Set when Discord queued its authorisation modal, cleared once connected.
let awaitingAuthorization = false;
// What the automatic update checks found: version strings, or null.
let appUpdate = null;
let firmwareUpdate = null;
// The Discord account, `{ name, avatarUrl }`, or null before the first connection. Kept while
// disconnected, so the panel still says whose Discord it is waiting for.
let discordUser = null;

const SERIAL_STATES = ['connected', 'disconnected', 'firmwareOutdated', 'appOutdated'];

const toAbout = { label: 'status.update', onClick: () => selectTab('acerca') };
const toDiscord = { label: 'status.setUp', onClick: () => selectTab('discord') };

function discordState() {
  if (discordConnected) return 'connected';
  return hasCredentials ? 'waiting' : 'unset';
}

/// Shows a problem, or once it is gone, a short confirmation if the problem had been shown.
function report(id, problem, fixed) {
  if (problem) {
    // Errors wait out the start; warnings (updates) are never a false alarm.
    if (!(starting && problem.kind === 'error')) {
      showNotice(id, problem);
    }
  } else if (noticeKind(id) === 'error' || noticeKind(id) === 'warning') {
    showNotice(id, { ...fixed, kind: 'success', transient: true });
  } else if (noticeKind(id) !== 'success') {
    hideNotice(id);
  }
}

function deviceProblem() {
  const values = { version: firmwareUpdate ?? '', firmware: serial.firmware ?? '', board: serial.board ?? '' };
  const base = { icon: 'device', values };
  switch (serial.state) {
    case 'disconnected':
      return { ...base, kind: 'error', title: 'status.deviceDisconnected', detail: 'status.deviceDisconnectedDetail' };
    case 'firmwareOutdated':
      return { ...base, kind: 'warning', title: 'status.deviceFirmwareOutdated', detail: 'status.deviceFirmwareOutdatedDetail', action: toAbout };
    case 'appOutdated':
      return { ...base, kind: 'warning', title: 'status.deviceAppOutdated', detail: 'status.deviceAppOutdatedDetail', action: toAbout };
    default:
      return firmwareUpdate
        ? { ...base, kind: 'warning', icon: 'update', title: 'status.deviceUpdateAvailable', detail: 'status.deviceUpdateAvailableDetail', action: toAbout }
        : null;
  }
}

function discordProblem() {
  const base = { icon: 'discord', values: { name: discordUser?.name ?? '' } };
  switch (discordState()) {
    case 'unset':
      return { ...base, kind: 'error', title: 'status.discordUnset', detail: 'status.discordUnsetDetail', action: toDiscord };
    case 'waiting':
      if (awaitingAuthorization) {
        return { ...base, kind: 'warning', title: 'status.discordAwaiting', detail: 'status.discordAwaitingDetail' };
      }
      return discordRunning
        ? { ...base, kind: 'error', title: 'status.discordDisconnected', detail: 'status.discordWaiting' }
        : { ...base, kind: 'error', title: 'status.discordClosed', detail: 'status.discordClosedDetail' };
    default:
      return null;
  }
}

function renderNotices() {
  if (serial) {
    const values = { firmware: serial.firmware ?? '', board: serial.board ?? '' };
    report('device', deviceProblem(), { icon: 'device', title: 'status.deviceConnected', detail: 'status.deviceConnectedDetail', values });
  }
  if (hasCredentials !== null) {
    const name = discordUser?.name;
    report('discord', discordProblem(), {
      icon: 'discord',
      title: 'status.discordConnectedTitle',
      detail: name ? 'status.discordConnectedAs' : null,
      values: { name: name ?? '' },
    });
  }
  if (appUpdate) {
    showNotice('app-update', { kind: 'warning', icon: 'update', title: 'status.appUpdateAvailable', detail: 'status.appUpdateAvailableDetail', values: { version: appUpdate }, action: toAbout });
  } else {
    hideNotice('app-update');
  }
}

function renderUser() {
  const discord = discordState();
  userCard.dataset.state = discord;
  userName.textContent = discord === 'unset' ? 'DS2000' : discordUser?.name ?? t('status.discordName');
  const avatar = discord !== 'unset' && discordUser?.avatarUrl ? discordUser.avatarUrl : LOGO;
  if (userAvatar.getAttribute('src') !== avatar) {
    userAvatar.src = avatar;
  }
  userStatus.textContent = {
    connected: t('status.discordConnected'),
    waiting: awaitingAuthorization
      ? t('status.discordAwaiting')
      : discordRunning ? t('status.discordWaiting') : t('status.discordClosed'),
    unset: t('status.discordUnset'),
  }[discord];
  userCard.title = userStatus.textContent;

  discordBadge.hidden = hasCredentials === null || discord === 'connected';
  const deviceNeedsUpdate = serial && (serial.state === 'firmwareOutdated' || serial.state === 'appOutdated' || firmwareUpdate);
  aboutBadge.hidden = !deviceNeedsUpdate && !appUpdate;
}

function render() {
  renderNotices();
  renderUser();
}

/// Told by the app updater which newer version of the app was found, or null.
export function setAppUpdateAvailable(version) {
  appUpdate = version ?? null;
  render();
}

/// Told by the firmware updater which newer firmware was found, or null.
export function setFirmwareUpdateAvailable(version) {
  firmwareUpdate = version ?? null;
  render();
}

/// Told by the Discord tab whenever it learns whether credentials are stored.
export function setHasCredentials(value) {
  hasCredentials = Boolean(value);
  render();
}

userCard.addEventListener('click', () => selectTab('discord'));

listen('DISCORD_CONNECTION_STATUS_EVENT', event => {
  discordConnected = Boolean(event.payload);
  if (discordConnected) {
    awaitingAuthorization = false;
  }
  render();
});

listen('DISCORD_RUNNING_EVENT', event => {
  discordRunning = Boolean(event.payload);
  render();
});

listen('DISCORD_USER_EVENT', event => {
  const payload = event.payload;
  discordUser = payload && typeof payload.name === 'string'
    ? { name: payload.name, avatarUrl: payload.avatarUrl ?? null }
    : null;
  render();
});

// Offline, or the CDN refused it: the logo rather than a broken image.
userAvatar.addEventListener('error', () => {
  if (userAvatar.getAttribute('src') !== LOGO) {
    userAvatar.src = LOGO;
  }
});

listen('DISCORD_AWAITING_AUTHORIZATION_EVENT', () => {
  awaitingAuthorization = true;
  render();
});

listen('SERIAL_CONNECTION_STATUS_EVENT', event => {
  const payload = event.payload;
  if (!payload || typeof payload !== 'object' || !SERIAL_STATES.includes(payload.state)) {
    console.error('Unexpected serial status payload:', payload);
    return;
  }
  serial = {
    state: payload.state,
    firmware: payload.firmware ?? null,
    board: payload.board ?? null,
  };
  render();
});

setTimeout(() => {
  starting = false;
  render();
}, STARTUP_GRACE_MS);

// Written from JavaScript, so the markup-driven pass cannot reach it.
onLanguageChange(renderUser);
