import { describe, expect, it } from 'vitest'
import { chosenDevices, deviceRows, isLastChecked } from '../deviceChoices.js'

const headphones = { id: '{hp}', name: 'Headphones' };
const speakers = { id: '{sp}', name: 'Speakers' };
const hdmi = { id: '{hdmi}', name: 'HDMI' };

const state = rows => rows.map(row => [row.id, row.checked, row.unplugged]);

describe('audio device rows', () => {
  it('start with every device ticked', () => {
    expect(state(deviceRows([headphones, speakers], []))).toEqual([
      ['{hp}', true, false],
      ['{sp}', true, false],
    ]);
  });

  it("tick Discord's devices that were chosen", () => {
    expect(state(deviceRows([headphones, speakers, hdmi], [speakers, hdmi]))).toEqual([
      ['{hp}', false, false],
      ['{sp}', true, false],
      ['{hdmi}', true, false],
    ]);
  });

  it('keep a chosen device Discord no longer lists, so it can be unticked', () => {
    expect(state(deviceRows([headphones], [speakers, headphones]))).toEqual([
      ['{hp}', true, false],
      ['{sp}', true, true],
    ]);
  });

  it('show only the choices while Discord is not connected', () => {
    expect(deviceRows([], [speakers]).map(row => row.id)).toEqual(['{sp}']);
  });

  it('never let the last ticked device be unticked', () => {
    const rows = deviceRows([headphones, speakers], [speakers]);
    expect(isLastChecked(rows, rows[1])).toBe(true);
    expect(isLastChecked(rows, rows[0])).toBe(false);
    const all = deviceRows([headphones, speakers], []);
    expect(all.some(row => isLastChecked(all, row))).toBe(false);
  });

  it('store the ticked rows in the order shown, without the row state', () => {
    const rows = deviceRows([headphones, speakers, hdmi], [hdmi, headphones]);
    expect(chosenDevices(rows)).toEqual([headphones, hdmi]);
  });

  it('store every device ticked as the default, so new devices join too', () => {
    expect(chosenDevices(deviceRows([headphones, speakers], [headphones, speakers]))).toEqual([]);
    // An unplugged choice is explicit: dropping it would lose it.
    expect(chosenDevices(deviceRows([headphones], [headphones, speakers]))).toEqual([headphones, speakers]);
  });
});
