import { invoke } from '@tauri-apps/api/core'
import { t, onLanguageChange } from './i18n.js'
import {
  rgbToHex, hexToRgb, sameColor, toPercent, previewPeriod,
  rgbToHsv, hsvToRgb, wheelPointToHueSaturation, hueSaturationToWheelPoint,
} from './color.js'

// RGB tab: mode, brightness, speed and one colour per LED, with a live preview of both LEDs.
// The device is write-only for RGB, so the preview shows what was asked for rather than
// pretending to report back what it lit.

// Named by the translation key of their accessible label. Full-intensity colours, eleven hues 30°
// apart plus white, because that is what an LED shows best: mixed pastels wash out on it.
const PALETTE = [
  ['red', '#FF0000'], ['orange', '#FF8000'], ['yellow', '#FFFF00'], ['lime', '#80FF00'],
  ['green', '#00FF00'], ['spring', '#00FF80'], ['cyan', '#00FFFF'], ['azure', '#0080FF'],
  ['blue', '#0000FF'], ['violet', '#8000FF'], ['magenta', '#FF00FF'], ['white', '#FFFFFF'],
].map(([name, hex]) => ({ name, color: hexToRgb(hex) }));

// The rainbow's dots on its mode button.
const RAINBOW_DOTS = ['#FF0000', '#00FF00', '#0000FF'];

const state = {
  mode: 'rainbow',
  brightness: 128,
  speed: 128,
  led1: { red: 255, green: 0, blue: 0 },
  led2: { red: 255, green: 0, blue: 0 },
};

const preview = document.getElementById('led-preview');
const previewLeds = { led1: document.getElementById('preview-led1'), led2: document.getElementById('preview-led2') };
const modeButtons = document.querySelectorAll('.mode-button');
const brightnessSlider = document.getElementById('brightness-slider');
const brightnessValue = document.getElementById('brightness-value');
const speedSlider = document.getElementById('speed-slider');
const speedValue = document.getElementById('speed-value');
const speedRow = document.getElementById('speed-row');
const ledGrid = document.getElementById('led-grid');
const ledCards = [...document.querySelectorAll('.led-card')].map(card => ({
  led: card.dataset.led,
  swatches: card.querySelector('.swatches'),
  hexSwatch: card.querySelector('.hex-swatch'),
  hexInput: card.querySelector('.hex-input'),
  picker: card.querySelector('.color-picker'),
  wheel: card.querySelector('.color-wheel'),
  wheelThumb: card.querySelector('.color-wheel-thumb'),
}));

// The open colour picker: which LED, and its colour as hue, saturation and value. Kept as HSV
// rather than derived from the RGB each time, or the hue would be lost whenever the saturation
// or the value reached zero. Null while closed.
let picker = null;
// Arrow keys on the wheel: degrees of hue and steps of saturation per press.
const HUE_STEP = 5;
const SATURATION_STEP = 0.05;

function send() {
  invoke('serial_set_rgb', {
    request: {
      mode: state.mode,
      brightness: state.brightness,
      speed: state.speed,
      led1: state.led1,
      led2: state.led2,
    },
  }).catch(error => console.error('Could not send the RGB configuration:', error));
}

function render() {
  modeButtons.forEach(button => {
    button.setAttribute('aria-pressed', String(button.dataset.mode === state.mode));
    const dots = button.querySelectorAll('.mode-dots span');
    const colors = {
      rainbow: RAINBOW_DOTS,
      fixed: [rgbToHex(state.led1), rgbToHex(state.led1), rgbToHex(state.led1)],
      breathing: [rgbToHex(state.led1), null, rgbToHex(state.led1)],
    }[button.dataset.mode];
    dots.forEach((dot, index) => {
      if (colors[index]) {
        dot.style.setProperty('--dot', colors[index]);
      } else {
        dot.style.removeProperty('--dot');
      }
    });
  });

  // Each control is hidden where the mode ignores it: showing a control the device ignores invites
  // the conclusion that it is broken.
  speedRow.hidden = state.mode === 'fixed';
  ledGrid.hidden = state.mode === 'rainbow';

  brightnessSlider.value = state.brightness;
  speedSlider.value = state.speed;
  brightnessValue.textContent = toPercent(state.brightness);
  speedValue.textContent = toPercent(state.speed);

  preview.dataset.mode = state.mode;
  preview.style.setProperty('--glow', String(0.2 + 0.8 * (state.brightness / 255)));
  preview.style.setProperty('--period', `${previewPeriod(state.speed).toFixed(2)}s`);
  for (const led of ['led1', 'led2']) {
    // The rainbow animates the hue from red, so its starting colour is fixed.
    previewLeds[led].style.setProperty('--led', state.mode === 'rainbow' ? '#FF0000' : rgbToHex(state[led]));
  }

  ledCards.forEach(card => {
    const color = state[card.led];
    const hex = rgbToHex(color);
    card.hexSwatch.style.setProperty('--swatch', hex);
    // Leave the field alone while it is being typed into, or the caret jumps.
    if (document.activeElement !== card.hexInput) {
      card.hexInput.value = hex;
      card.hexInput.removeAttribute('aria-invalid');
    }
    card.swatches.querySelectorAll('.swatch').forEach((swatch, index) => {
      swatch.setAttribute('aria-pressed', String(sameColor(PALETTE[index].color, color)));
    });

    const open = picker?.led === card.led;
    card.picker.hidden = !open;
    card.hexSwatch.setAttribute('aria-expanded', String(open));
    if (open) {
      const { hue, saturation } = picker.hsv;
      const point = hueSaturationToWheelPoint(hue, saturation);
      card.wheelThumb.style.left = `${point.left}%`;
      card.wheelThumb.style.top = `${point.top}%`;
      card.wheelThumb.style.setProperty('--swatch', hex);
      card.wheel.setAttribute('aria-valuenow', String(Math.round(hue)));
      card.wheel.setAttribute('aria-valuetext', hex);
    }
  });
}

function update(changes) {
  Object.assign(state, changes);
  // A colour picked elsewhere (palette, hex) moves the open picker to it.
  if (picker && changes[picker.led]) {
    picker.hsv = rgbToHsv(changes[picker.led]);
  }
  render();
  send();
}

// Gap between the swatch and the picker, and the least it keeps from the window's edges.
const PICKER_GAP = 8;

/// Puts the picker under the swatch, or above it when there is no room below, inside the window.
function placePicker(card) {
  const swatch = card.hexSwatch.getBoundingClientRect();
  const { width, height } = card.picker.getBoundingClientRect();
  const below = swatch.bottom + PICKER_GAP;
  const top = below + height <= window.innerHeight - PICKER_GAP ? below : swatch.top - PICKER_GAP - height;
  const left = Math.min(swatch.left, window.innerWidth - width - PICKER_GAP);
  card.picker.style.top = `${Math.max(PICKER_GAP, top)}px`;
  card.picker.style.left = `${Math.max(PICKER_GAP, left)}px`;
}

function openPicker(card) {
  picker = { led: card.led, hsv: rgbToHsv(state[card.led]) };
  render();
  placePicker(card);
  card.wheel.focus();
}

function closePicker({ returnFocus = false } = {}) {
  const card = ledCards.find(c => c.led === picker?.led);
  picker = null;
  render();
  if (returnFocus && card) {
    card.hexSwatch.focus();
  }
}

/// Applies the picker's colour, always at full value: the wheel only picks hue and saturation, and
/// how bright the LEDs shine is the Brightness slider's job. While dragging only the preview follows; the device is written
/// once, on release, so a drag is a single write instead of hundreds.
function applyPicker(hsvChanges, { commit }) {
  Object.assign(picker.hsv, hsvChanges, { value: 1 });
  state[picker.led] = hsvToRgb(picker.hsv);
  render();
  if (commit) {
    send();
  }
}

function pickFromPointer(card, event, commit) {
  const box = card.wheel.getBoundingClientRect();
  const x = ((event.clientX - box.left) / box.width) * 2 - 1;
  const y = ((event.clientY - box.top) / box.height) * 2 - 1;
  applyPicker(wheelPointToHueSaturation(x, y), { commit });
}

modeButtons.forEach(button => {
  button.addEventListener('click', () => update({ mode: button.dataset.mode }));
});

// The value updates while dragging; the device only on release, so a drag is one write.
brightnessSlider.addEventListener('input', () => {
  state.brightness = Number(brightnessSlider.value);
  render();
});
brightnessSlider.addEventListener('change', send);

speedSlider.addEventListener('input', () => {
  state.speed = Number(speedSlider.value);
  render();
});
speedSlider.addEventListener('change', send);

ledCards.forEach(card => {
  PALETTE.forEach(({ color }) => {
    const swatch = document.createElement('button');
    swatch.type = 'button';
    swatch.className = 'swatch';
    swatch.style.setProperty('--swatch', rgbToHex(color));
    swatch.addEventListener('click', () => update({ [card.led]: { ...color } }));
    card.swatches.append(swatch);
  });

  card.hexInput.addEventListener('input', () => {
    const color = hexToRgb(card.hexInput.value);
    card.hexInput.setAttribute('aria-invalid', String(!color));
    if (color) {
      update({ [card.led]: color });
    }
  });
  // Normalise whatever is left in the field once it loses focus.
  card.hexInput.addEventListener('blur', render);

  card.hexSwatch.addEventListener('click', () => {
    if (picker?.led === card.led) {
      closePicker();
    } else {
      openPicker(card);
    }
  });

  card.wheel.addEventListener('pointerdown', event => {
    card.wheel.setPointerCapture(event.pointerId);
    pickFromPointer(card, event, false);
  });
  card.wheel.addEventListener('pointermove', event => {
    if (card.wheel.hasPointerCapture(event.pointerId)) {
      pickFromPointer(card, event, false);
    }
  });
  card.wheel.addEventListener('pointerup', event => pickFromPointer(card, event, true));

  card.wheel.addEventListener('keydown', event => {
    const { hue, saturation } = picker.hsv;
    const changes = {
      ArrowLeft: { hue: (hue - HUE_STEP + 360) % 360 },
      ArrowRight: { hue: (hue + HUE_STEP) % 360 },
      ArrowUp: { saturation: Math.min(1, saturation + SATURATION_STEP) },
      ArrowDown: { saturation: Math.max(0, saturation - SATURATION_STEP) },
    }[event.key];
    if (changes) {
      event.preventDefault();
      applyPicker(changes, { commit: true });
    }
  });

  card.picker.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      event.stopPropagation();
      closePicker({ returnFocus: true });
    }
  });
});

// It is placed once, against the swatch, so scrolling the tab away from the swatch closes it.
document.querySelector('#rgb .tab-body').addEventListener('scroll', () => {
  if (picker) closePicker();
});

// A click anywhere outside the open picker and its swatch closes it.
document.addEventListener('pointerdown', event => {
  if (!picker) return;
  const card = ledCards.find(c => c.led === picker.led);
  if (!card.picker.contains(event.target) && !card.hexSwatch.contains(event.target)) {
    closePicker();
  }
});

function translateLabels() {
  ledCards.forEach(card => {
    const led = t(`rgb.${card.led}`);
    card.swatches.setAttribute('aria-label', t('rgb.palette', { led }));
    card.hexInput.setAttribute('aria-label', t('rgb.hex', { led }));
    card.hexSwatch.setAttribute('aria-label', t('rgb.openPicker', { led }));
    card.picker.setAttribute('aria-label', t('rgb.picker', { led }));
    card.wheel.setAttribute('aria-label', t('rgb.wheel', { led }));
    card.swatches.querySelectorAll('.swatch').forEach((swatch, index) => {
      swatch.setAttribute('aria-label', t(`rgb.colors.${PALETTE[index].name}`));
    });
  });
}

// Start from what is saved, not from the defaults above: otherwise the first touch of any control
// would send the defaults for all the others and overwrite the saved configuration.
async function load() {
  try {
    const saved = await invoke('rgb_config');
    state.mode = saved.mode;
    state.brightness = saved.brightness;
    state.speed = saved.speed;
    // Rainbow stores no colours, so the defaults stay for whenever another mode is picked.
    if (saved.led1) state.led1 = saved.led1;
    if (saved.led2) state.led2 = saved.led2;
  } catch (error) {
    console.error('Could not read the saved RGB configuration:', error);
  }
  render();
}

onLanguageChange(translateLabels);
render();
load();
