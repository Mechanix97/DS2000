import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-shell'
import { t, ready, language, onLanguageChange } from './i18n.js'
import { parseChangelog, parseInline, releasedUpTo, unseenEntries } from './changelog.js'

// Bundled at build time, so the notes are the ones this version shipped with and need no
// network. The release workflow refuses a tag whose version has no entry in it.
import changelog from '../../CHANGELOG.md?raw'

// "What's new": on the first start after an update, the CHANGELOG.md entries since the version
// the user had. The backend says which versions (commands.rs, `release_notes_unseen`). The same
// dialog shows the whole history from About → Changelog.

const dialog = document.getElementById('whats-new');
const title = document.getElementById('whats-new-title');
const intro = document.getElementById('whats-new-intro');
const body = document.getElementById('whats-new-body');

// Keep a Changelog's section names, translated; any other heading is shown as written.
const SECTIONS = ['added', 'changed', 'deprecated', 'removed', 'fixed', 'security'];

let entries = [];
let running = null;
// 'update' after an update, 'history' from the About tab: only the heading and intro differ.
let mode = 'update';

function inline(text) {
  return parseInline(text).map(run => {
    if (run.type === 'text') return document.createTextNode(run.text);
    const element = document.createElement(run.type === 'link' ? 'button' : run.type);
    element.textContent = run.text;
    if (run.type === 'link') {
      element.type = 'button';
      element.className = 'link-button';
      element.addEventListener('click', () => open(run.href));
    }
    return element;
  });
}

function sectionName(text) {
  const key = text.toLowerCase();
  return SECTIONS.includes(key) ? t(`whatsNew.sections.${key}`) : text;
}

// Rendered from the entries rather than kept, so a language change redraws the headings.
function render() {
  const history = mode === 'history';
  title.textContent = t(history ? 'whatsNew.historyTitle' : 'whatsNew.title');
  intro.textContent = t(history ? 'whatsNew.historyIntro' : 'whatsNew.intro', { version: running });
  // The changelog is written in English only.
  if (language() !== 'en') intro.textContent += ` ${t('whatsNew.englishOnly')}`;

  body.replaceChildren(...entries.map(entry => {
    const article = document.createElement('article');
    article.className = 'whats-new-entry';
    const heading = document.createElement('h3');
    heading.className = 'whats-new-version';
    heading.textContent = entry.date ? `${entry.version} · ${entry.date}` : entry.version;
    article.append(heading);
    for (const block of entry.blocks) {
      if (block.type === 'heading') {
        const section = document.createElement('h4');
        section.className = 'card-title';
        section.textContent = sectionName(block.text);
        article.append(section);
      } else if (block.type === 'paragraph') {
        const paragraph = document.createElement('p');
        paragraph.append(...inline(block.text));
        article.append(paragraph);
      } else {
        const list = document.createElement('ul');
        list.append(...block.items.map(item => {
          const li = document.createElement('li');
          li.append(...inline(item));
          return li;
        }));
        article.append(list);
      }
    }
    return article;
  }));
}

async function markSeen() {
  try {
    await invoke('release_notes_seen');
  } catch (error) {
    console.warn('Could not record the release notes as seen:', error);
  }
}

async function showUnseen() {
  let unseen;
  try {
    [unseen, running] = await Promise.all([invoke('release_notes_unseen'), invoke('app_version')]);
  } catch (error) {
    console.warn('Could not read which release notes are unseen:', error);
    return;
  }
  if (!unseen) return;

  mode = 'update';
  entries = unseenEntries(parseChangelog(changelog), unseen.since, running);
  if (entries.length === 0) {
    // A downgrade, or a build whose changelog lacks its own entry: nothing to show.
    await markSeen();
    return;
  }
  render();
  dialog.showModal();
}

async function showHistory() {
  try {
    running ??= await invoke('app_version');
  } catch (error) {
    console.warn('Could not read the app version:', error);
    return;
  }
  mode = 'history';
  entries = releasedUpTo(parseChangelog(changelog), running);
  render();
  if (!dialog.open) dialog.showModal();
  body.scrollTop = 0;
}

document.getElementById('whats-new-open').addEventListener('click', showHistory);

// Escape closes the dialog too, and is as good as the button.
dialog.addEventListener('close', markSeen);
document.getElementById('whats-new-close').addEventListener('click', () => dialog.close());

onLanguageChange(() => {
  if (dialog.open) render();
});
ready.then(showUnseen);
