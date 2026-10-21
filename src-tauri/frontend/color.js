// Colour conversions for the RGB tab. Kept apart from the DOM so they can be tested on their own.

/// `{ red, green, blue }` (0–255 each) to `#RRGGBB`.
export function rgbToHex({ red, green, blue }) {
  return '#' + [red, green, blue].map(channel => channel.toString(16).padStart(2, '0')).join('').toUpperCase();
}

/// `#RRGGBB` or `RRGGBB`, any case, surrounding spaces allowed, to `{ red, green, blue }`; `null`
/// for anything else, so a half-typed value is never sent to the device.
export function hexToRgb(text) {
  const match = /^#?([0-9a-f]{6})$/i.exec(String(text).trim());
  if (!match) {
    return null;
  }
  const value = parseInt(match[1], 16);
  return { red: (value >> 16) & 0xff, green: (value >> 8) & 0xff, blue: value & 0xff };
}

/// Whether two colours are the same.
export function sameColor(a, b) {
  return a.red === b.red && a.green === b.green && a.blue === b.blue;
}

/// Slider position (0–255) to a percentage, for the value shown next to it.
export function toPercent(value) {
  return `${Math.round((value / 255) * 100)}%`;
}

/// Speed (0–255, faster to the right) to the period of one preview animation cycle, in seconds.
/// The device's own timing is not reported back, so this only has to read as slow to fast.
export function previewPeriod(speed) {
  return 1.2 + ((255 - speed) / 255) * 6;
}

/// `{ red, green, blue }` to `{ hue, saturation, value }`: hue in degrees (0–360), the other two
/// from 0 to 1. A grey has no hue, so it comes back as 0.
export function rgbToHsv({ red, green, blue }) {
  const [r, g, b] = [red / 255, green / 255, blue / 255];
  const max = Math.max(r, g, b);
  const delta = max - Math.min(r, g, b);
  let hue = 0;
  if (delta) {
    if (max === r) hue = ((g - b) / delta) % 6;
    else if (max === g) hue = (b - r) / delta + 2;
    else hue = (r - g) / delta + 4;
    hue = (hue * 60 + 360) % 360;
  }
  return { hue, saturation: max ? delta / max : 0, value: max };
}

/// The inverse of `rgbToHsv`, rounded to whole channel values.
export function hsvToRgb({ hue, saturation, value }) {
  const channel = n => {
    const k = (n + hue / 60) % 6;
    return Math.round((value - value * saturation * Math.max(0, Math.min(k, 4 - k, 1))) * 255);
  };
  return { red: channel(5), green: channel(3), blue: channel(1) };
}

/// A point on the colour wheel, `x` and `y` from -1 to 1 with the centre at 0 and y growing down,
/// to `{ hue, saturation }`. Red is at the right and hues grow clockwise, matching the wheel's
/// `conic-gradient(from 90deg, ...)`; outside the circle clamps to its edge.
export function wheelPointToHueSaturation(x, y) {
  const hue = ((Math.atan2(y, x) * 180) / Math.PI + 360) % 360;
  return { hue, saturation: Math.min(1, Math.hypot(x, y)) };
}

/// Where a hue and saturation sit on the wheel, as percentages of its box for the thumb.
export function hueSaturationToWheelPoint(hue, saturation) {
  const angle = (hue * Math.PI) / 180;
  return { left: 50 + Math.cos(angle) * saturation * 50, top: 50 + Math.sin(angle) * saturation * 50 };
}
