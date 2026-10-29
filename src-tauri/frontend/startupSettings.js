import { invoke } from '@tauri-apps/api/core'
import { t, ready, language, setLanguage } from './i18n.js'

const startWithWindows = document.getElementById('start-with-windows');
const startMinimized = document.getElementById('start-minimized');
const desktopNotifications = document.getElementById('desktop-notifications');
const languageSelector = document.getElementById('language-selector');
const feedback = document.getElementById('startup-feedback');

function showFeedback(message, kind) {
  feedback.textContent = message;
  feedback.classList.remove('ok', 'error');
  if (kind) {
    feedback.classList.add(kind);
  }
}

// Start minimised only applies when Windows starts the app, so it is only editable while that is
// on. Its stored value is kept, so turning start with Windows back on restores it.
function syncStartMinimized() {
  startMinimized.disabled = !startWithWindows.checked;
}

// Registering with Windows can fail on its own, so the checkboxes are only trusted once the
// backend confirms. On failure they are put back to what is actually stored rather than left
// showing a setting that never took effect.
async function save() {
  const preferences = {
    startWithWindows: startWithWindows.checked,
    startMinimized: startMinimized.checked,
  };

  try {
    await invoke('set_startup_preferences', { preferences });
    showFeedback(t('settings.saved'), 'ok');
  } catch (error) {
    showFeedback(String(error), 'error');
    await load();
  }
}

async function load() {
  try {
    const preferences = await invoke('startup_preferences');
    startWithWindows.checked = preferences.startWithWindows;
    startMinimized.checked = preferences.startMinimized;
    syncStartMinimized();
  } catch (error) {
    showFeedback(t('settings.loadFailed'), 'error');
    console.error('startup_preferences failed:', error);
  }
}

startWithWindows.addEventListener('change', () => {
  syncStartMinimized();
  save();
});
startMinimized.addEventListener('change', save);

desktopNotifications.addEventListener('change', async () => {
  try {
    await invoke('set_desktop_notifications', { enabled: desktopNotifications.checked });
  } catch (error) {
    desktopNotifications.checked = !desktopNotifications.checked;
    console.error('set_desktop_notifications failed:', error);
  }
});

languageSelector.addEventListener('change', async event => {
  try {
    await setLanguage(event.target.value);
  } catch (error) {
    // Put the selector back: the language did not change, so it must not claim otherwise.
    languageSelector.value = language();
    showFeedback(t('settings.languageFailed'), 'error');
    console.error('set_ui_language failed:', error);
  }
});

async function loadNotifications() {
  try {
    desktopNotifications.checked = await invoke('desktop_notifications');
  } catch (error) {
    console.error('desktop_notifications failed:', error);
  }
}

ready.then(() => {
  languageSelector.value = language();
  return Promise.all([load(), loadNotifications()]);
});
