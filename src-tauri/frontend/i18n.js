import { invoke } from '@tauri-apps/api/core'

import en from './translations/en.json'
import es from './translations/es.json'

// Bundled rather than fetched: Vite inlines them, so a translation is never missing because a
// request failed, and there is no flash of untranslated text while one is in flight.
const TRANSLATIONS = { en, es };

// English is the fallback for a missing key, so it is the one file that has to be complete.
const FALLBACK = 'en';

let current = FALLBACK;
const listeners = new Set();

function lookup(catalog, key) {
  return key.split('.').reduce((node, part) => (node == null ? undefined : node[part]), catalog);
}

/// Fills `{name}` placeholders, every occurrence of each.
function interpolate(text, values) {
  return Object.entries(values).reduce((result, [name, value]) => result.replaceAll(`{${name}}`, value), text);
}

/// Translates a dotted key, e.g. `discord.connect`, filling placeholders from `values`, e.g.
/// `t('about.install', { version })`.
export function t(key, values = {}) {
  const translated = lookup(TRANSLATIONS[current], key);
  if (typeof translated === 'string') {
    return interpolate(translated, values);
  }

  const fallback = lookup(TRANSLATIONS[FALLBACK], key);
  if (typeof fallback === 'string') {
    console.warn(`Missing ${current} translation for "${key}", falling back to ${FALLBACK}`);
    return interpolate(fallback, values);
  }

  // Showing the key is ugly but debuggable, and better than a blank label.
  console.error(`No translation for "${key}" in any language`);
  return key;
}

export function language() {
  return current;
}

/// Translates everything under `root` that is marked up for it.
///
/// Attribute-driven rather than a template engine: the markup stays plain HTML, and an element
/// that nobody marked up simply keeps its literal text instead of disappearing.
export function applyTranslations(root = document) {
  root.querySelectorAll('[data-i18n]').forEach(element => {
    element.textContent = t(element.dataset.i18n);
  });
  root.querySelectorAll('[data-i18n-placeholder]').forEach(element => {
    element.placeholder = t(element.dataset.i18nPlaceholder);
  });
  // SVG <title> elements are what a screen reader announces for the mic and headset icons.
  root.querySelectorAll('[data-i18n-title]').forEach(element => {
    element.textContent = t(element.dataset.i18nTitle);
  });
  root.querySelectorAll('[data-i18n-tooltip]').forEach(element => {
    element.title = t(element.dataset.i18nTooltip);
  });
  root.querySelectorAll('[data-i18n-aria-label]').forEach(element => {
    element.setAttribute('aria-label', t(element.dataset.i18nAriaLabel));
  });
  document.documentElement.lang = current;
}

/// Registers a callback for text that is not in the markup, such as the connection status, which
/// is rewritten from JavaScript whenever the backend reports a change.
export function onLanguageChange(callback) {
  listeners.add(callback);
}

function activate(tag) {
  current = TRANSLATIONS[tag] ? tag : FALLBACK;
  applyTranslations();
  listeners.forEach(callback => callback(current));
}

/// Switches language and persists the choice.
export async function setLanguage(tag) {
  // The backend also renders the tray menu, so it has to be told rather than merely informed
  // after the fact.
  await invoke('set_ui_language', { language: tag });
  activate(tag);
}

/// Adopts the language the backend resolved.
///
/// The backend decides, not the webview: it builds the tray menu before any window exists when
/// the app starts minimised, so it cannot wait to be told what the language is.
async function initI18n() {
  try {
    activate(await invoke('ui_language'));
  } catch (error) {
    console.error('Could not read the UI language, falling back to English:', error);
    activate(FALLBACK);
  }
}

/// Resolves once the language is known.
///
/// Every module shares this one promise, so awaiting it before rendering text keeps modules from
/// racing the language and briefly showing the wrong one.
export const ready = initI18n();
