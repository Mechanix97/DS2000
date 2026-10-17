import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { t, ready, onLanguageChange } from './i18n.js'
import { openSitePage } from './links.js'
import { selectTab } from './tabs.js'
import { setHasCredentials } from './statusPanel.js'

// Discord tab: three steps, create the application, paste its credentials, connect. Each step
// shows whether it is done, so a returning user sees at a glance that nothing is missing.

const stepApp = document.getElementById('step-app');
const stepCredentials = document.getElementById('step-credentials');
const stepConnect = document.getElementById('step-connect');
const stepConnectTitle = document.getElementById('step-connect-title');
const stepConnectText = document.getElementById('step-connect-text');
const clientIdInput = document.getElementById('discord-client-id');
const clientSecretInput = document.getElementById('discord-client-secret');
const secretSaved = document.getElementById('discord-secret-saved');
const replaceButton = document.getElementById('discord-replace-secret');
const redirectUriLabel = document.getElementById('discord-redirect-uri');
const copyRedirectButton = document.getElementById('discord-copy-redirect');
const saveButton = document.getElementById('discord-save-button');
const cancelButton = document.getElementById('discord-cancel-button');
const clearButton = document.getElementById('discord-clear-button');
const unlinkConfirm = document.getElementById('discord-unlink-confirm');
const unlinkYes = document.getElementById('discord-unlink-yes');
const unlinkNo = document.getElementById('discord-unlink-no');
const guideLink = document.getElementById('discord-setup-guide');
const feedback = document.getElementById('discord-feedback');

let storedClientId = '';
let hasClientSecret = false;
let connected = false;
let awaitingAuthorization = false;
// Whether Discord itself is open; unknown counts as open.
let discordRunning = true;
// The account's display name, once a connection reported it.
let userName = null;
// Replacing the stored credentials: the secret field is shown again and saving overwrites both.
let replacing = false;
// Showing the "are you sure" box under the unlink button.
let confirmingUnlink = false;

function showFeedback(message, kind) {
  feedback.textContent = message;
  feedback.classList.remove('ok', 'error');
  if (kind) {
    feedback.classList.add(kind);
  }
}

// Rendered from the state rather than set piecemeal, so a language change redraws it correctly.
function render() {
  const editing = !hasClientSecret || replacing;

  stepApp.dataset.state = hasClientSecret ? 'done' : 'active';
  stepCredentials.dataset.state = editing ? 'active' : 'done';
  stepConnect.dataset.state = connected ? 'done' : hasClientSecret && !replacing ? 'active' : 'pending';

  // The secret is never returned by the backend once stored, so a saved one is shown as saved
  // rather than as an empty field that reads as if it were missing.
  secretSaved.hidden = editing;
  clientSecretInput.hidden = !editing;
  clientIdInput.readOnly = !editing;

  const phase = connected ? 'connected'
    : editing ? 'unset'
      : awaitingAuthorization ? 'awaiting'
        : discordRunning ? 'waiting' : 'closed';
  stepConnectTitle.textContent = phase === 'connected' && userName
    ? t('discord.step3Title.connectedAs', { name: userName })
    : t(`discord.step3Title.${phase}`);
  stepConnectText.textContent = t(`discord.step3Body.${phase}`);

  saveButton.hidden = !editing;
  cancelButton.hidden = !replacing;
  clearButton.hidden = !hasClientSecret || replacing || confirmingUnlink;
  unlinkConfirm.hidden = !hasClientSecret || replacing || !confirmingUnlink;
}

function applyStatus(status) {
  if (status.redirect_uri) {
    redirectUriLabel.textContent = status.redirect_uri;
  }
  storedClientId = status.client_id ?? '';
  clientIdInput.value = storedClientId;
  hasClientSecret = status.has_client_secret;
  connected = status.connected;
  setHasCredentials(hasClientSecret);
  render();
  return status;
}

async function refreshStatus() {
  try {
    return applyStatus(await invoke('discord_credentials_status'));
  } catch (error) {
    console.error('Could not read the Discord status:', error);
    showFeedback(t('discord.statusFailed'), 'error');
    return null;
  }
}

saveButton.addEventListener('click', async () => {
  const clientId = clientIdInput.value.trim();
  const clientSecret = clientSecretInput.value.trim();

  if (!clientId || !clientSecret) {
    showFeedback(t('discord.missingFields'), 'error');
    return;
  }

  saveButton.disabled = true;
  showFeedback(t('discord.saving'), null);

  try {
    await invoke('discord_set_credentials', { clientId, clientSecret });
    clientSecretInput.value = '';
    replacing = false;
    showFeedback(t('discord.saved'), 'ok');
    await refreshStatus();
  } catch (error) {
    // The backend returns actionable messages, so surface them verbatim.
    showFeedback(String(error), 'error');
  } finally {
    saveButton.disabled = false;
  }
});

replaceButton.addEventListener('click', () => {
  replacing = true;
  showFeedback('', null);
  render();
  clientSecretInput.focus();
});

cancelButton.addEventListener('click', () => {
  replacing = false;
  clientIdInput.value = storedClientId;
  clientSecretInput.value = '';
  showFeedback('', null);
  render();
});

function setConfirming(value) {
  confirmingUnlink = value;
  render();
  (value ? unlinkNo : clearButton).focus();
}

clearButton.addEventListener('click', () => setConfirming(true));
unlinkNo.addEventListener('click', () => setConfirming(false));

unlinkYes.addEventListener('click', async () => {
  unlinkYes.disabled = true;
  try {
    await invoke('discord_clear_credentials');
    clientSecretInput.value = '';
    confirmingUnlink = false;
    showFeedback(t('discord.unlinked'), null);
    await refreshStatus();
  } catch (error) {
    showFeedback(String(error), 'error');
  } finally {
    unlinkYes.disabled = false;
  }
});

copyRedirectButton.addEventListener('click', async () => {
  try {
    await navigator.clipboard.writeText(redirectUriLabel.textContent);
    showFeedback(t('discord.copied'), 'ok');
  } catch (error) {
    console.warn('Could not copy the redirect URI:', error);
  }
});

listen('DISCORD_CONNECTION_STATUS_EVENT', event => {
  connected = Boolean(event.payload);
  if (connected) {
    awaitingAuthorization = false;
  }
  render();
});

listen('DISCORD_RUNNING_EVENT', event => {
  discordRunning = Boolean(event.payload);
  render();
});

listen('DISCORD_USER_EVENT', event => {
  userName = typeof event.payload?.name === 'string' ? event.payload.name : null;
  render();
});

// Discord draws its authorisation modal inside its own window, so with Discord minimised to the
// tray the request is queued and invisible. Without this notice the app just looks stuck.
listen('DISCORD_AWAITING_AUTHORIZATION_EVENT', () => {
  awaitingAuthorization = true;
  render();
});

guideLink.addEventListener('click', () => openSitePage('discordSetup'));

// The feedback line is left alone: it describes something that just happened, so restating it in
// another language would be a lie about when it happened.
onLanguageChange(render);

// Open on the Discord tab when there is nothing configured yet: without credentials the app
// cannot do anything useful, so that is where a new user needs to land.
ready.then(refreshStatus).then(status => {
  if (status && !status.has_client_secret) {
    selectTab('discord');
  }
});
