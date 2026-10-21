import { describe, expect, it } from 'vitest'
import {
  hexToRgb, hsvToRgb, hueSaturationToWheelPoint, previewPeriod, rgbToHex, rgbToHsv, sameColor, toPercent,
  wheelPointToHueSaturation,
} from '../color.js'

describe('hex colours', () => {
  it('round-trips through RGB', () => {
    for (const hex of ['#000000', '#FFFFFF', '#5865F2', '#EB459E']) {
      expect(rgbToHex(hexToRgb(hex))).toBe(hex);
    }
  });

  it('accepts what people type', () => {
    expect(hexToRgb('5865f2')).toEqual({ red: 0x58, green: 0x65, blue: 0xf2 });
    expect(hexToRgb('  #5865F2 ')).toEqual({ red: 0x58, green: 0x65, blue: 0xf2 });
  });

  it('rejects anything half typed, so it is never sent to the device', () => {
    for (const text of ['', '#', '#5865F', '#5865F2A', 'blurple', '#GGGGGG']) {
      expect(hexToRgb(text)).toBeNull();
    }
  });

  it('pads single-digit channels', () => {
    expect(rgbToHex({ red: 1, green: 2, blue: 3 })).toBe('#010203');
  });
});

describe('sliders', () => {
  it('shows the ends as 0% and 100%', () => {
    expect(toPercent(0)).toBe('0%');
    expect(toPercent(255)).toBe('100%');
  });

  it('runs the preview faster as the speed goes up', () => {
    expect(previewPeriod(255)).toBeLessThan(previewPeriod(128));
    expect(previewPeriod(128)).toBeLessThan(previewPeriod(0));
  });

  it('compares colours by value', () => {
    expect(sameColor({ red: 1, green: 2, blue: 3 }, { red: 1, green: 2, blue: 3 })).toBe(true);
    expect(sameColor({ red: 1, green: 2, blue: 3 }, { red: 1, green: 2, blue: 4 })).toBe(false);
  });
});

describe('colour wheel', () => {
  it('round-trips colours through HSV', () => {
    for (const hex of ['#5865F2', '#EB459E', '#FEE75C', '#000000', '#FFFFFF', '#808080']) {
      expect(rgbToHex(hsvToRgb(rgbToHsv(hexToRgb(hex))))).toBe(hex);
    }
  });

  it('puts red at the right and grows the hue clockwise', () => {
    expect(wheelPointToHueSaturation(1, 0)).toEqual({ hue: 0, saturation: 1 });
    // y grows down, so straight down is a quarter turn clockwise.
    expect(wheelPointToHueSaturation(0, 1).hue).toBeCloseTo(90);
    expect(wheelPointToHueSaturation(-1, 0).hue).toBeCloseTo(180);
  });

  it('is white at the centre and clamps outside the circle to its edge', () => {
    expect(wheelPointToHueSaturation(0, 0).saturation).toBe(0);
    expect(wheelPointToHueSaturation(2, 2).saturation).toBe(1);
  });

  it('places the thumb where a click would pick the same colour', () => {
    for (const [hue, saturation] of [[0, 1], [120, 0.5], [275, 0.8]]) {
      const { left, top } = hueSaturationToWheelPoint(hue, saturation);
      const picked = wheelPointToHueSaturation(left / 50 - 1, top / 50 - 1);
      expect(picked.hue).toBeCloseTo(hue);
      expect(picked.saturation).toBeCloseTo(saturation);
    }
  });
});
