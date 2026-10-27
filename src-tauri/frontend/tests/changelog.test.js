import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { compareVersions, parseChangelog, parseInline, releasedUpTo, unseenEntries } from '../changelog.js'

const SAMPLE = `# Changelog

Intro text.

## [Unreleased]

- Not released yet.

## [0.5.0] - 2026-11-01

Serial protocol 1.

### Added

- A **new** thing that wraps
  onto a second line.
- Another one.

## [0.4.1] - 2026-10-23

### Fixed

- A fix.

## [0.4.0] - 2026-10-21

- Old.

[Unreleased]: https://example.com/compare
[0.5.0]: https://example.com/0.5.0
`;

describe('changelog', () => {
  const entries = parseChangelog(SAMPLE);

  it('reads every released version, newest first, and skips Unreleased', () => {
    expect(entries.map(entry => entry.version)).toEqual(['0.5.0', '0.4.1', '0.4.0']);
    expect(entries[0].date).toBe('2026-11-01');
  });

  it('keeps paragraphs, section headings and wrapped list items', () => {
    expect(entries[0].blocks).toEqual([
      { type: 'heading', text: 'Added' },
      { type: 'list', items: ['A **new** thing that wraps onto a second line.', 'Another one.'] },
    ]);
  });

  it('stops at the link definitions', () => {
    expect(entries.at(-1).blocks).toEqual([{ type: 'list', items: ['Old.'] }]);
  });

  it('leaves out the serial protocol sentence, and only that', () => {
    const paragraphs = markdown => parseChangelog(`## [1.0.0] - 2026-01-01\n\n${markdown}\n`)[0].blocks;
    expect(paragraphs('Serial protocol 1: pairs with DS2000-Firmware 0.2.1.')).toEqual([]);
    expect(paragraphs('Serial protocol 1.')).toEqual([]);
    expect(paragraphs('First release. Serial protocol 1: pairs with DS2000 app 0.2.0.'))
      .toEqual([{ type: 'paragraph', text: 'First release.' }]);
    expect(paragraphs('Works with version 0.2.1.')).toEqual([{ type: 'paragraph', text: 'Works with version 0.2.1.' }]);
  });

  it('splits bold, code and https links out of a line, and nothing else', () => {
    expect(parseInline('Open **About** with `F1`, see [the site](https://x.y) or [this](http://x.y).')).toEqual([
      { type: 'text', text: 'Open ' },
      { type: 'strong', text: 'About' },
      { type: 'text', text: ' with ' },
      { type: 'code', text: 'F1' },
      { type: 'text', text: ', see ' },
      { type: 'link', text: 'the site', href: 'https://x.y' },
      { type: 'text', text: ' or [this](http://x.y).' },
    ]);
  });

  it('compares versions numerically', () => {
    expect(compareVersions('0.10.0', '0.9.9')).toBeGreaterThan(0);
    expect(compareVersions('1.0.0', '1.0.0')).toBe(0);
  });

  it('shows the versions after the one the user had, up to the running one', () => {
    const versions = (since, running) => unseenEntries(entries, since, running).map(entry => entry.version);
    expect(versions('0.4.0', '0.5.0')).toEqual(['0.5.0', '0.4.1']);
    expect(versions('0.4.0', '0.4.1')).toEqual(['0.4.1']);
    expect(versions(null, '0.5.0')).toEqual(['0.5.0']);
    expect(versions('0.5.0', '0.4.1')).toEqual([]);
  });

  it('lists the whole history up to the running version for the About tab', () => {
    expect(releasedUpTo(entries, '0.4.1').map(entry => entry.version)).toEqual(['0.4.1', '0.4.0']);
  });

  it('reads the real CHANGELOG.md: every release has notes', () => {
    const real = parseChangelog(readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8'));
    expect(real.length).toBeGreaterThan(0);
    for (const entry of real) {
      expect(entry.blocks.length, entry.version).toBeGreaterThan(0);
    }
  });
});
