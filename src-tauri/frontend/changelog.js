// Reads CHANGELOG.md for the "What's new" dialog. Only the Markdown the changelog uses: a version
// heading per release, `###` section headings, paragraphs, `-` lists with indented continuation
// lines, and **bold**, `code` and https links inside them. No DOM here, so it is tested on its own.

const VERSION_HEADING = /^## \[(\d+\.\d+\.\d+)\](?: - (\S+))?/;
const INLINE = /\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\((https:\/\/[^)\s]+)\)/g;

/// `[{ version, date, blocks }]`, newest first as in the file. `[Unreleased]` and the link
/// definitions at the bottom are left out. A block is `{ type: 'heading' | 'paragraph', text }`
/// or `{ type: 'list', items }`.
export function parseChangelog(markdown) {
  const entries = [];
  let entry = null;
  let block = null;

  for (const line of markdown.split(/\r?\n/)) {
    if (line.startsWith('## ')) {
      const match = VERSION_HEADING.exec(line);
      entry = match ? { version: match[1], date: match[2] ?? null, blocks: [] } : null;
      if (entry) entries.push(entry);
      block = null;
    } else if (/^\[[^\]]+\]: /.test(line)) {
      break;
    } else if (!entry) {
      continue;
    } else if (line.trim() === '') {
      block = null;
    } else if (line.startsWith('### ')) {
      entry.blocks.push({ type: 'heading', text: line.slice(4).trim() });
      block = null;
    } else if (line.startsWith('- ')) {
      if (block?.type !== 'list') {
        block = { type: 'list', items: [] };
        entry.blocks.push(block);
      }
      block.items.push(line.slice(2).trim());
    } else if (block?.type === 'list') {
      block.items[block.items.length - 1] += ` ${line.trim()}`;
    } else if (block?.type === 'paragraph') {
      block.text += ` ${line.trim()}`;
    } else {
      block = { type: 'paragraph', text: line.trim() };
      entry.blocks.push(block);
    }
  }
  return entries;
}

/// Splits a line into `{ type: 'text' | 'strong' | 'code', text }` and `{ type: 'link', text,
/// href }` runs. Only https links are recognised; anything else stays text.
export function parseInline(text) {
  const runs = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    if (match.index > last) runs.push({ type: 'text', text: text.slice(last, match.index) });
    if (match[1] !== undefined) {
      runs.push({ type: 'strong', text: match[1] });
    } else if (match[2] !== undefined) {
      runs.push({ type: 'code', text: match[2] });
    } else {
      runs.push({ type: 'link', text: match[3], href: match[4] });
    }
    last = match.index + match[0].length;
  }
  if (last < text.length) runs.push({ type: 'text', text: text.slice(last) });
  return runs;
}

/// Orders `x.y.z` versions numerically.
export function compareVersions(a, b) {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) {
    if (pa[i] !== pb[i]) return pa[i] - pb[i];
  }
  return 0;
}

/// Every released entry up to and including `running`, newest first: the whole history this
/// version knows about.
export function releasedUpTo(entries, running) {
  return entries.filter(entry => compareVersions(entry.version, running) <= 0);
}

/// The entries the user has not seen: newer than `since`, up to and including `running`. With no
/// `since` (the previous version did not record it) only the running version's entry.
export function unseenEntries(entries, since, running) {
  return entries.filter(entry => compareVersions(entry.version, running) <= 0 &&
    (since ? compareVersions(entry.version, since) > 0 : entry.version === running));
}
