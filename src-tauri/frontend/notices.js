import { t, onLanguageChange } from './i18n.js'

// Notifications stacked above the user panel. Each source (the device, Discord, updates) owns one
// slot by id, so a new state replaces its previous notice instead of piling up. Problems stay
// until they are fixed; a fix shows briefly and goes away on its own.

const container = document.getElementById('notices');

/// How long a transient notice (something that just got fixed) stays up.
const TRANSIENT_MS = 4000;
/// Matches the leave animation in styles.css.
const LEAVE_MS = 180;

// Static markup, never built from data.
const ICONS = {
  device: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="M9 2v6M15 2v6M7 8h10v4a5 5 0 0 1-10 0zM12 17v5"/></svg>',
  discord: '<svg viewBox="0 0 16 16" fill="currentColor"><path d="M13.55 2.9A13.2 13.2 0 0 0 10.29 1.9c-.14.25-.3.58-.41.83a12.2 12.2 0 0 0-3.66 0A8 8 0 0 0 5.8 1.9c-1.12.19-2.22.53-3.26 1.01C.36 6.02-.21 9.05.07 12.03a13.3 13.3 0 0 0 4 2.02c.32-.43.6-.87.85-1.35a9 9 0 0 1-1.27-.6l.31-.24c2.62 1.2 5.45 1.2 8.04 0l.31.24a8 8 0 0 1-1.27.6c.25.48.53.92.85 1.35a13.2 13.2 0 0 0 4-2.02c.33-3.45-.56-6.45-2.37-9.11zM5.35 10.21c-.79 0-1.44-.72-1.44-1.61s.64-1.61 1.44-1.61c.81 0 1.45.73 1.44 1.61 0 .89-.64 1.61-1.44 1.61zm5.32 0c-.79 0-1.44-.72-1.44-1.61s.64-1.61 1.44-1.61c.81 0 1.45.73 1.44 1.61 0 .89-.63 1.61-1.44 1.61z"/></svg>',
  update: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg>',
};

/// id -> { spec, element, timer }
const notices = new Map();

function render(notice) {
  const { spec, element } = notice;
  element.dataset.kind = spec.kind;
  element.querySelector('.notice-icon').innerHTML = ICONS[spec.icon];
  element.querySelector('.notice-title').textContent = t(spec.title, spec.values);
  element.querySelector('.notice-detail').textContent = spec.detail ? t(spec.detail, spec.values) : '';
  const action = element.querySelector('.notice-action');
  action.hidden = !spec.action;
  if (spec.action) {
    action.textContent = t(spec.action.label);
  }
}

function create(id) {
  const element = document.createElement('div');
  element.className = 'notice';
  element.setAttribute('role', 'status');
  element.innerHTML =
    '<span class="notice-icon" aria-hidden="true"></span>' +
    '<span class="notice-text"><span class="notice-title"></span><span class="notice-detail"></span></span>' +
    '<button class="primary-button small notice-action" type="button"></button>';
  element.querySelector('.notice-action').addEventListener('click', () => notices.get(id)?.spec.action?.onClick());
  // The container stacks bottom-up, so the newest is appended and shows on top.
  container.append(element);
  return { element, timer: null };
}

/// Shows or updates the notice in slot `id`.
///
/// `spec`: `{ kind: 'error' | 'warning' | 'success', icon: 'device' | 'discord' | 'update',
/// title, detail?, values?, action?: { label, onClick }, transient? }`, with `title`, `detail`
/// and `action.label` as translation keys filled from `values`.
export function showNotice(id, spec) {
  let notice = notices.get(id);
  if (!notice || notice.leaving) {
    notice?.element.remove();
    notice = create(id);
    notices.set(id, notice);
  }
  clearTimeout(notice.timer);
  notice.spec = spec;
  render(notice);
  notice.timer = spec.transient ? setTimeout(() => hideNotice(id), TRANSIENT_MS) : null;
}

/// Removes the notice in slot `id`, if there is one.
export function hideNotice(id) {
  const notice = notices.get(id);
  if (!notice || notice.leaving) return;
  clearTimeout(notice.timer);
  notice.leaving = true;
  notice.element.classList.add('leaving');
  setTimeout(() => {
    notice.element.remove();
    if (notices.get(id) === notice) notices.delete(id);
  }, LEAVE_MS);
}

/// The kind of the notice showing in slot `id`, or null when there is none.
export function noticeKind(id) {
  const notice = notices.get(id);
  return notice && !notice.leaving ? notice.spec.kind : null;
}

onLanguageChange(() => notices.forEach(notice => !notice.leaving && render(notice)));
