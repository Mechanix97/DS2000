import { describe, expect, it } from 'vitest'
import { chosenDevices, deviceRows } from '../deviceChoices.js'

const headphones = { id: '{hp}', name: 'Headphones' };
const speakers = { id: '{sp}', name: 'Speakers' };
const hdmi = { id: '{hdmi}', name: 'HDMI' };

describe('audio device rows', () => {
  it("tick Discord's devices that were chosen", () => {
    const rows = deviceRows([headphones, speakers, hdmi], [speakers]);
    expect(rows.map(row => [row.id, row.checked, row.unplugged])).toEqual([
      ['{hp}', false, false],
      ['{sp}', true, false],
      ['{hdmi}', false, false],
    ]);
  });

  it('keep a chosen device Discord no longer lists, so it can be unticked', () => {
    const rows = deviceRows([headphones], [speakers, headphones]);
    expect(rows.map(row => [row.id, row.checked, row.unplugged])).toEqual([
      ['{hp}', true, false],
      ['{sp}', true, true],
    ]);
  });

  it('show only the choices while Discord is not connected', () => {
    expect(deviceRows([], [speakers]).map(row => row.id)).toEqual(['{sp}']);
  });

  it('store the ticked rows in the order shown, without the row state', () => {
    const rows = deviceRows([headphones, speakers, hdmi], [hdmi, headphones]);
    expect(chosenDevices(rows)).toEqual([headphones, hdmi]);
  });
});
