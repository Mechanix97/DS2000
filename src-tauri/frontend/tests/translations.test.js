import { readFileSync, readdirSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import en from '../translations/en.json'
import es from '../translations/es.json'

const root = new URL('..', import.meta.url);

/// Every leaf of a catalog as `[dotted.key, text]`.
function entries(catalog, prefix = '') {
  return Object.entries(catalog).flatMap(([key, value]) =>
    typeof value === 'object' ? entries(value, `${prefix}${key}.`) : [[`${prefix}${key}`, value]]
  );
}

const enEntries = new Map(entries(en));
const esEntries = new Map(entries(es));

function placeholders(text) {
  return [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
}

describe('translations', () => {
  it('have the same keys in English and Spanish', () => {
    expect([...esEntries.keys()].sort()).toEqual([...enEntries.keys()].sort());
  });

  it('keep the same placeholders in both languages', () => {
    for (const [key, text] of enEntries) {
      expect(placeholders(esEntries.get(key) ?? ''), key).toEqual(placeholders(text));
    }
  });

  it('cover every key the markup asks for', () => {
    const html = readFileSync(new URL('index.html', root), 'utf8');
    const keys = [...html.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g)].map(match => match[1]);
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(enEntries.has(key), key).toBe(true);
    }
  });

  it('cover every literal key the scripts ask for', () => {
    const scripts = readdirSync(root).filter(file => file.endsWith('.js') && !file.endsWith('.config.js'));
    for (const file of scripts) {
      const source = readFileSync(new URL(file, root), 'utf8');
      for (const [, key] of source.matchAll(/\bt\('([\w.]+)'/g)) {
        expect(enEntries.has(key), `${file}: ${key}`).toBe(true);
      }
    }
  });

  it('spell the product DS2000, without a space', () => {
    for (const [key, text] of [...enEntries, ...esEntries]) {
      expect(text, key).not.toMatch(/DS 2000/);
    }
  });
});
