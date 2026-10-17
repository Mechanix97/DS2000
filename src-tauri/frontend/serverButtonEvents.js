import { open } from '@tauri-apps/plugin-shell';
import { openSitePage } from './links.js';

// Pages on the project site follow the app's language; the social networks have only one.
const SITE_LINKS = {
  'DS2000-button': 'ds2000',
  'Mechardo-Labs-button': 'home',
  serviceTerms: 'termsOfService',
  privacyPolicy: 'privacyPolicy',
};

const EXTERNAL_LINKS = {
  'instagram-button': 'https://www.instagram.com/mechardo3d/',
  'twitter-button': 'https://x.com/MechardoLabs',
  'youtube-button': 'https://www.youtube.com/@MechardoLabs',
  'discord-invite-button': 'https://discord.gg/VtbFAGJe86',
};

for (const [id, page] of Object.entries(SITE_LINKS)) {
  document.getElementById(id).addEventListener('click', () => openSitePage(page));
}

for (const [id, url] of Object.entries(EXTERNAL_LINKS)) {
  document.getElementById(id).addEventListener('click', () => open(url));
}
