import { open } from '@tauri-apps/plugin-shell'
import { language } from './i18n.js'

// Pages on the project site. Its routes carry the language as their first segment
// (`/{lang}/...`), so every link is built with the app's current language and the page opens in
// the language the user is already reading.

const SITE = 'https://www.mechardo3d.xyz';

export const SITE_PAGES = {
  home: '',
  ds2000: '/ds2000',
  discordSetup: '/ds2000/discord-setup',
  termsOfService: '/ds2000/terms-of-service',
  privacyPolicy: '/ds2000/privacy-policy',
};

/// The URL of a site page in the given language, e.g. `siteUrl('discordSetup', 'es')`.
export function siteUrl(page, lang) {
  return `${SITE}/${lang}${SITE_PAGES[page]}`;
}

/// Opens a site page in the browser, in the app's current language.
export function openSitePage(page) {
  return open(siteUrl(page, language()));
}
