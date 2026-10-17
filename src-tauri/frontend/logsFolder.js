import { invoke } from '@tauri-apps/api/core'
import { t } from './i18n.js'

// About tab: open the folder with the log files, so they can be attached to a bug report. The
// files themselves are written by the backend (main.rs, init_logging).

const statusLabel = document.getElementById('logs-status');
const button = document.getElementById('logs-button');

button.addEventListener('click', async () => {
  try {
    await invoke('open_logs_folder');
    statusLabel.classList.remove('error');
    statusLabel.textContent = t('logs.description');
  } catch (error) {
    console.warn('Could not open the logs folder:', error);
    statusLabel.classList.add('error');
    statusLabel.textContent = t('logs.openFailed');
  }
});
