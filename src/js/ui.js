// Shared UI helpers: HTML templating, icons, formatting, toasts, modals,
// confirm/prompt dialogs, dropdown menus and a few reusable fragments.
import { errorText, fileSrc } from './tauri.js';

// ==========================================================================
// DOM & templating
// ==========================================================================

export const $ = (selector, root = document) => root.querySelector(selector);
export const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

/** Escape text for HTML (text content and attribute values). */
export const esc = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => ESCAPES[c]);

class Raw {
  constructor(html) {
    this.html = html;
  }
  toString() {
    return this.html;
  }
}

/** Mark a string as trusted HTML so `html` won't escape it. */
export const raw = (value) => new Raw(String(value ?? ''));

function renderValue(value) {
  if (value == null || value === false) return '';
  if (value instanceof Raw) return value.html;
  if (Array.isArray(value)) return value.map(renderValue).join('');
  return esc(value);
}

/**
 * Tagged template that escapes every interpolated value unless it came from
 * `raw()` or another `html` call. Arrays are joined; null/false render nothing.
 *
 *   html`<p class="x">${userText}</p>${condition && html`<b>yes</b>`}`
 */
export function html(strings, ...values) {
  let out = strings[0];
  for (let i = 0; i < values.length; i++) out += renderValue(values[i]) + strings[i + 1];
  return new Raw(out);
}

/** Build a single element from an HTML string (or `html` result). */
export function el(markup) {
  const template = document.createElement('template');
  template.innerHTML = String(markup).trim();
  return template.content.firstElementChild;
}

/** Replace an element's contents with markup (a string, `html` result, or an array of them). */
export function setHtml(node, markup) {
  node.innerHTML = Array.isArray(markup) ? renderValue(markup) : String(markup);
  wireImages(node);
  return node;
}

/**
 * Images with `data-fallback` swap to that markup when they fail to load
 * (inline `onerror` handlers are blocked by the CSP).
 */
export function wireImages(root) {
  for (const img of root.querySelectorAll('img[data-fallback]')) {
    if (img.dataset.wired) continue;
    img.dataset.wired = '1';
    const fail = () => {
      const fallback = img.dataset.fallback;
      if (fallback) img.replaceWith(el(fallback) ?? document.createTextNode(''));
      else img.remove();
    };
    img.addEventListener('error', fail, { once: true });
    if (img.complete && img.naturalWidth === 0 && img.src) fail();
  }
}

/** Debounce a function. */
export function debounce(fn, ms = 250) {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  };
}

// ==========================================================================
// Icons (Lucide, MIT)
// ==========================================================================

const ICONS = {
  play: '<polygon points="6 3 20 12 6 21 6 3"/>',
  square: '<rect width="14" height="14" x="5" y="5" rx="2"/>',
  plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  search: '<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>',
  home: '<path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><polyline points="9 22 9 12 15 12 15 22"/>',
  layers:
    '<path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z"/><path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65"/><path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65"/>',
  compass:
    '<circle cx="12" cy="12" r="10"/><polygon points="16.24 7.76 14.12 14.12 7.76 16.24 9.88 9.88 16.24 7.76"/>',
  settings:
    '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>',
  download:
    '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>',
  upload:
    '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" x2="12" y1="3" y2="15"/>',
  folder:
    '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  'folder-input':
    '<path d="M2 9V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H20a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-1"/><path d="M2 13h10"/><path d="m9 16 3-3-3-3"/>',
  package:
    '<path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/>',
  trash:
    '<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>',
  pencil: '<path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/>',
  copy: '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  external:
    '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>',
  refresh:
    '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
  shield:
    '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>',
  alert:
    '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
  info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  'check-circle': '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  'x-circle': '<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>',
  'chevron-down': '<path d="m6 9 6 6 6-6"/>',
  'chevron-up': '<path d="m18 15-6-6-6 6"/>',
  'chevron-right': '<path d="m9 18 6-6-6-6"/>',
  'chevron-left': '<path d="m15 18-6-6 6-6"/>',
  'arrow-left': '<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>',
  'arrow-right': '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
  'arrow-up-circle': '<circle cx="12" cy="12" r="10"/><path d="m16 12-4-4-4 4"/><path d="M12 16V8"/>',
  more: '<circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/>',
  user: '<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
  'log-out':
    '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" x2="9" y1="12" y2="12"/>',
  shirt:
    '<path d="M20.38 3.46 16 2a4 4 0 0 1-8 0L3.62 3.46a2 2 0 0 0-1.34 2.23l.58 3.47a1 1 0 0 0 .99.84H6v10c0 1.1.9 2 2 2h8a2 2 0 0 0 2-2V10h2.15a1 1 0 0 0 .99-.84l.58-3.47a2 2 0 0 0-1.34-2.23z"/>',
  server:
    '<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/>',
  globe:
    '<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>',
  image:
    '<rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>',
  archive:
    '<rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/>',
  terminal: '<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>',
  clock: '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>',
  coffee:
    '<path d="M10 2v2"/><path d="M14 2v2"/><path d="M16 8a1 1 0 0 1 1 1v8a4 4 0 0 1-4 4H7a4 4 0 0 1-4-4V9a1 1 0 0 1 1-1h14a4 4 0 1 1 0 8h-1"/><path d="M6 2v2"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
  moon: '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>',
  monitor:
    '<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>',
  gamepad:
    '<line x1="6" x2="10" y1="12" y2="12"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="15" x2="15.01" y1="13" y2="13"/><line x1="18" x2="18.01" y1="11" y2="11"/><rect width="20" height="12" x="2" y="6" rx="2"/>',
  sparkles:
    '<path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/>',
  heart:
    '<path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/>',
  link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
  'rotate-ccw': '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
  eye: '<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/><circle cx="12" cy="12" r="3"/>',
  'eye-off':
    '<path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49"/><path d="M14.084 14.158a3 3 0 0 1-4.242-4.242"/><path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143"/><path d="m2 2 20 20"/>',
  power: '<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>',
  grid: '<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>',
  list: '<line x1="8" x2="21" y1="6" y2="6"/><line x1="8" x2="21" y1="12" y2="12"/><line x1="8" x2="21" y1="18" y2="18"/><line x1="3" x2="3.01" y1="6" y2="6"/><line x1="3" x2="3.01" y1="12" y2="12"/><line x1="3" x2="3.01" y1="18" y2="18"/>',
  save: '<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"/><path d="M7 3v4a1 1 0 0 0 1 1h7"/>',
  key: '<circle cx="7.5" cy="15.5" r="5.5"/><path d="m21 2-9.6 9.6"/><path d="m15.5 7.5 3 3L22 7l-3-3"/>',
  zap: '<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>',
  filter: '<polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3"/>',
  star: '<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>',
  'hard-drive':
    '<line x1="22" x2="2" y1="12" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/><line x1="6" x2="6.01" y1="16" y2="16"/><line x1="10" x2="10.01" y1="16" y2="16"/>',
  cpu: '<rect width="16" height="16" x="4" y="4" rx="2"/><rect width="6" height="6" x="9" y="9" rx="1"/><path d="M15 2v2"/><path d="M15 20v2"/><path d="M2 15h2"/><path d="M2 9h2"/><path d="M20 15h2"/><path d="M20 9h2"/><path d="M9 2v2"/><path d="M9 20v2"/>',
  wand: '<path d="m21.64 3.64-1.28-1.28a1.21 1.21 0 0 0-1.72 0L2.36 18.64a1.21 1.21 0 0 0 0 1.72l1.28 1.28a1.2 1.2 0 0 0 1.72 0L21.64 5.36a1.2 1.2 0 0 0 0-1.72"/><path d="m14 7 3 3"/>',
};

/** Inline SVG markup for a Lucide icon (as a trusted `raw` value). */
export function icon(name, extraClass = '') {
  const body = ICONS[name] ?? ICONS.info;
  const filled = name === 'play' || name === 'square' ? ' icon-fill' : '';
  return raw(
    `<svg class="icon${filled}${extraClass ? ` ${extraClass}` : ''}" viewBox="0 0 24 24" aria-hidden="true">${body}</svg>`,
  );
}

export const spinner = (extraClass = '') => raw(`<span class="spinner ${extraClass}"></span>`);

// ==========================================================================
// Formatting
// ==========================================================================

const COMPACT = new Intl.NumberFormat('en', { notation: 'compact' });

export function compactNumber(value) {
  if (!value) return '0';
  return value < 1000 ? String(value) : COMPACT.format(value);
}

export function formatBytes(bytes, decimals = 1) {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : decimals)} ${units[index]}`;
}

export function formatDuration(seconds) {
  if (!seconds) return 'Never';
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours === 0 ? `${minutes}m` : `${hours}h ${minutes}m`;
}

export function formatMemory(mb) {
  mb = Number(mb);
  return mb >= 1024 ? `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB` : `${mb} MB`;
}

export function formatDate(iso) {
  if (!iso) return '—';
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? '—'
    : date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

const RELATIVE = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
const RELATIVE_UNITS = [
  [31_536_000_000, 'year'],
  [2_592_000_000, 'month'],
  [604_800_000, 'week'],
  [86_400_000, 'day'],
  [3_600_000, 'hour'],
  [60_000, 'minute'],
];

/** `3 days ago`, `just now`, or `Never` for a missing timestamp. */
export function relativeTime(iso) {
  if (!iso) return 'Never';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return 'Unknown';
  const diff = Date.now() - then;
  if (diff < 60_000) return 'just now';
  for (const [threshold, unit] of RELATIVE_UNITS) {
    if (diff >= threshold) return RELATIVE.format(-Math.floor(diff / threshold), unit);
  }
  return 'just now';
}

const LOADER_LABELS = { neoforge: 'NeoForge', forge: 'Forge', fabric: 'Fabric', quilt: 'Quilt', vanilla: 'Vanilla' };
export const LOADERS = ['vanilla', 'fabric', 'quilt', 'forge', 'neoforge'];
export const loaderLabel = (loader) => LOADER_LABELS[loader] ?? 'Vanilla';

const PROJECT_TYPE_LABELS = {
  mod: 'Mod',
  modpack: 'Modpack',
  resourcepack: 'Resource Pack',
  shader: 'Shader',
  datapack: 'Datapack',
};
export const projectTypeLabel = (type) => PROJECT_TYPE_LABELS[type] ?? 'Mod';

export const severityLabel = (severity) =>
  ({ critical: 'Critical', warning: 'Warning' })[severity] ?? 'Info';

export function initials(name) {
  const cleaned = String(name ?? '').replace(/[^a-zA-Z0-9 ]/g, ' ').trim();
  if (!cleaned) return '?';
  const parts = cleaned.split(/\s+/);
  return parts.length === 1 ? parts[0].slice(0, 2).toUpperCase() : (parts[0][0] + parts[1][0]).toUpperCase();
}

// ==========================================================================
// Fragments
// ==========================================================================

/** Coloured badge for a loader id. */
export const loaderBadge = (loader) =>
  html`<span class="badge badge-${LOADER_LABELS[loader] ? loader : 'vanilla'}">${loaderLabel(loader)}</span>`;

/** Coloured badge for a project type. */
export const projectTypeBadge = (type) =>
  html`<span class="badge badge-${PROJECT_TYPE_LABELS[type] ? type : 'mod'}">${projectTypeLabel(type)}</span>`;

function gradientFor(id) {
  let hash = 0;
  for (const c of String(id)) hash = (hash * 31 + c.charCodeAt(0)) | 0;
  const hue = Math.abs(hash) % 360;
  return `linear-gradient(135deg, hsl(${hue} 70% 42%), hsl(${(hue + 45) % 360} 70% 26%))`;
}

const isImagePath = (value) => /^(?:[a-zA-Z]:[\\/]|\/|file:)/.test(value);

/**
 * An instance's icon: an image (local path or URL), an emoji, or a gradient
 * monogram. `size` is the edge length in px.
 */
export function instanceIcon({ icon: value, name, id }, size = 48) {
  const style = `width:${size}px;height:${size}px;border-radius:${Math.round(size / 4)}px`;
  const fallback = html`<div class="inst-icon" style="${style};background:${gradientFor(id)};font-size:${Math.max(11, Math.round(size / 3.2))}px">${initials(name)}</div>`;
  const trimmed = value?.trim();
  if (trimmed) {
    const isUrl = /^(https?:|data:|blob:)/i.test(trimmed);
    if (isUrl || isImagePath(trimmed)) {
      const src = isUrl ? trimmed : fileSrc(trimmed.replace(/^file:/, ''));
      return html`<div class="inst-icon" style="${style}"><img src="${src}" alt="" loading="lazy" referrerpolicy="no-referrer" data-fallback="${String(fallback)}"></div>`;
    }
    return html`<div class="inst-icon is-emoji" style="${style};background:${gradientFor(id)};font-size:${Math.round(size / 1.9)}px">${trimmed}</div>`;
  }
  return fallback;
}

/** A player's head: rendered by mc-heads.net, falling back to initials. */
export function playerHead(account, size = 36) {
  const style = `width:${size}px;height:${size}px`;
  const fallback = html`<div class="avatar" style="${style}">${initials(account?.username)}</div>`;
  if (!account?.uuid) return fallback;
  const src = `https://mc-heads.net/avatar/${encodeURIComponent(account.uuid)}/${size * 2}`;
  return html`<div class="avatar" style="${style}"><img src="${src}" alt="" data-fallback="${String(fallback)}"></div>`;
}

export function emptyState({ iconName = 'sparkles', title, description, actions = '' }) {
  return html`
    <div class="empty">
      <div class="empty-icon">${icon(iconName, 'icon-lg')}</div>
      <h3>${title}</h3>
      ${description && html`<p>${description}</p>`}
      ${actions && html`<div class="row-wrap">${actions}</div>`}
    </div>`;
}

export const loadingBlock = (label = 'Loading…') => html`<div class="loading-block">${spinner()} ${label}</div>`;

export function errorBlock(error, retryId) {
  return html`
    <div class="empty">
      <div class="empty-icon" style="background:rgba(255,77,109,.12);color:var(--red)">${icon('alert', 'icon-lg')}</div>
      <h3>Something went wrong</h3>
      <p class="selectable">${errorText(error)}</p>
      ${retryId && html`<div class="row-wrap"><button class="btn btn-outline btn-sm" id="${retryId}">${icon('refresh')} Try again</button></div>`}
    </div>`;
}

// ==========================================================================
// Toasts
// ==========================================================================

const toastTimers = new Map();
let toastCounter = 0;

/**
 * Show (or update, when `id` already exists) a toast.
 *   kind: 'info' | 'success' | 'warn' | 'error' | 'loading'
 *   duration: ms, 0 = until dismissed (loading toasts default to 0)
 *   actions: [{ label, onClick, primary }]
 * Returns the toast id.
 */
export function toast(message, { kind = 'info', description, duration, actions = [], id } = {}) {
  const container = $('#toasts');
  const toastId = id ?? `t${++toastCounter}`;
  const leading = kind === 'loading' ? spinner() : icon(
    { success: 'check-circle', error: 'x-circle', warn: 'alert', info: 'info' }[kind] ?? 'info',
  );

  let node = container.querySelector(`[data-toast-id="${CSS.escape(toastId)}"]`);
  const markup = html`
    <div class="toast toast-${kind}" data-toast-id="${toastId}">
      ${leading}
      <div class="toast-text">
        <div class="toast-title">${message}</div>
        ${description && html`<div class="toast-desc">${description}</div>`}
        ${actions.length > 0 &&
        html`<div class="toast-actions">${actions.map(
          (action, index) =>
            html`<button class="btn btn-sm ${action.primary === false ? 'btn-outline' : 'btn-primary'}" data-action="${index}">${action.label}</button>`,
        )}</div>`}
      </div>
      <button class="toast-x" aria-label="Dismiss">${icon('x', 'icon-sm')}</button>
    </div>`;

  const fresh = el(markup);
  fresh.querySelector('.toast-x').addEventListener('click', () => dismissToast(toastId));
  fresh.querySelectorAll('[data-action]').forEach((button) =>
    button.addEventListener('click', () => {
      const action = actions[Number(button.dataset.action)];
      dismissToast(toastId);
      action?.onClick?.();
    }),
  );

  if (node) {
    fresh.classList.add('show');
    node.replaceWith(fresh);
  } else {
    container.appendChild(fresh);
    requestAnimationFrame(() => fresh.classList.add('show'));
  }

  clearTimeout(toastTimers.get(toastId));
  const life = duration ?? (kind === 'loading' || actions.length ? 0 : kind === 'error' ? 9000 : 4500);
  if (life > 0) toastTimers.set(toastId, setTimeout(() => dismissToast(toastId), life));
  return toastId;
}

export function dismissToast(id) {
  clearTimeout(toastTimers.get(id));
  toastTimers.delete(id);
  const node = $(`#toasts [data-toast-id="${CSS.escape(id)}"]`);
  if (!node) return;
  node.classList.remove('show');
  setTimeout(() => node.remove(), 220);
}

/** Toast an error from a failed call. */
export const toastError = (error, title) =>
  title ? toast(title, { kind: 'error', description: errorText(error) }) : toast(errorText(error), { kind: 'error' });

// ==========================================================================
// Modals
// ==========================================================================

const modalStack = [];

/**
 * Open a modal. Modals stack; Escape and the backdrop close the top one.
 *   title, description: header text
 *   body: markup for the body
 *   footer: markup for the footer (buttons with `data-close` close the modal)
 *   size: 'sm' | 'md' | 'lg' | 'xl'
 *   dismissable: false keeps it open on Escape/backdrop (e.g. while busy)
 *   onClose: called once when the modal closes, however it closes
 * Returns { root, body, foot, close, setDismissable }.
 */
export function openModal({ title, description, body = '', footer = '', size = 'md', dismissable = true, onClose } = {}) {
  const backdrop = el(html`
    <div class="modal-backdrop">
      <div class="modal modal-${size}" role="dialog" aria-modal="true">
        ${(title || description) &&
        html`<div class="modal-head">
          <div class="grow">
            ${title && html`<h2>${title}</h2>`}
            ${description && html`<p>${description}</p>`}
          </div>
          <button class="btn btn-ghost btn-icon btn-sm modal-close" data-close aria-label="Close">${icon('x')}</button>
        </div>`}
        <div class="modal-body">${raw(body)}</div>
        ${footer && html`<div class="modal-foot">${raw(footer)}</div>`}
      </div>
    </div>`);

  let closed = false;
  const entry = { dismissable, close };

  function close() {
    if (closed) return;
    closed = true;
    const index = modalStack.indexOf(entry);
    if (index >= 0) modalStack.splice(index, 1);
    backdrop.remove();
    onClose?.();
  }

  backdrop.addEventListener('mousedown', (event) => {
    if (event.target === backdrop && entry.dismissable) close();
  });
  backdrop.addEventListener('click', (event) => {
    if (event.target.closest('[data-close]') && entry.dismissable) close();
  });

  $('#modals').appendChild(backdrop);
  modalStack.push(entry);
  wireImages(backdrop);
  backdrop.querySelector('[autofocus], .modal-body input:not([type=checkbox]):not([type=range]), .modal-body textarea')?.focus();

  return {
    root: backdrop.querySelector('.modal'),
    body: backdrop.querySelector('.modal-body'),
    foot: backdrop.querySelector('.modal-foot'),
    close,
    /** While false, Escape, the backdrop and [data-close] buttons do nothing. */
    setDismissable(value) {
      entry.dismissable = value;
      backdrop.querySelectorAll('[data-close]').forEach((button) => (button.disabled = !value));
    },
    get closed() {
      return closed;
    },
  };
}

document.addEventListener('keydown', (event) => {
  if (event.key !== 'Escape') return;
  if (closeMenu()) return;
  const top = modalStack[modalStack.length - 1];
  if (top?.dismissable) top.close();
});

/** Ask a yes/no question. Resolves true when confirmed. */
export function confirmDialog({ title, message, confirmLabel = 'Confirm', danger = false, details }) {
  return new Promise((resolve) => {
    let answered = false;
    const modal = openModal({
      title,
      size: 'sm',
      body: html`
        <p class="muted" style="line-height:1.5">${message}</p>
        ${details && html`<div class="hint selectable">${details}</div>`}`,
      footer: html`
        <button class="btn btn-ghost" data-close>Cancel</button>
        <button class="btn ${danger ? 'btn-danger' : 'btn-primary'}" data-confirm>${confirmLabel}</button>`,
      onClose: () => !answered && resolve(false),
    });
    modal.foot.querySelector('[data-confirm]').addEventListener('click', () => {
      answered = true;
      resolve(true);
      modal.close();
    });
  });
}

/** Ask for a line of text. Resolves with the trimmed text, or null when cancelled. */
export function promptDialog({ title, label, value = '', placeholder = '', confirmLabel = 'Save', description }) {
  return new Promise((resolve) => {
    let answered = false;
    const modal = openModal({
      title,
      description,
      size: 'sm',
      body: html`
        <label class="field">
          <span>${label}</span>
          <input class="input" data-value value="${value}" placeholder="${placeholder}" autofocus spellcheck="false">
        </label>`,
      footer: html`
        <button class="btn btn-ghost" data-close>Cancel</button>
        <button class="btn btn-primary" data-confirm>${confirmLabel}</button>`,
      onClose: () => !answered && resolve(null),
    });
    const input = modal.body.querySelector('[data-value]');
    input.select();
    const submit = () => {
      const text = input.value.trim();
      if (!text) return;
      answered = true;
      resolve(text);
      modal.close();
    };
    modal.foot.querySelector('[data-confirm]').addEventListener('click', submit);
    input.addEventListener('keydown', (event) => event.key === 'Enter' && submit());
  });
}

// ==========================================================================
// Dropdown menus
// ==========================================================================

let openMenuState = null;

/** Close the open dropdown menu. Returns true when one was open. */
export function closeMenu() {
  if (!openMenuState) return false;
  openMenuState.node.remove();
  document.removeEventListener('mousedown', openMenuState.outside, true);
  openMenuState = null;
  return true;
}

/**
 * Show a dropdown next to `anchor`.
 *   items: [{ label, icon, onSelect, danger, disabled, trail }, { separator: true }, { header: 'Text' }]
 *   align: 'start' | 'end'; placement: 'below' | 'above'
 */
export function showMenu(anchor, items, { align = 'start', placement = 'below' } = {}) {
  const wasMine = openMenuState?.anchor === anchor;
  closeMenu();
  if (wasMine) return;

  const node = el(html`
    <div class="menu" role="menu">
      ${items.map((item, index) => {
        if (item.separator) return html`<div class="menu-sep"></div>`;
        if (item.header) return html`<div class="menu-label">${item.header}</div>`;
        return html`<button class="menu-item ${item.danger ? 'danger' : ''}" data-index="${index}" ${item.disabled ? raw('disabled') : ''}>
          ${item.icon ? icon(item.icon) : item.leading ? raw(item.leading) : ''}
          <span class="truncate">${item.label}</span>
          ${item.trail && html`<span class="trail">${item.trail}</span>`}
        </button>`;
      })}
    </div>`);
  document.body.appendChild(node);
  wireImages(node);

  const rect = anchor.getBoundingClientRect();
  const menuRect = node.getBoundingClientRect();
  let left = align === 'end' ? rect.right - menuRect.width : rect.left;
  left = Math.max(8, Math.min(left, window.innerWidth - menuRect.width - 8));
  let top = placement === 'above' ? rect.top - menuRect.height - 6 : rect.bottom + 6;
  if (top + menuRect.height > window.innerHeight - 8) top = rect.top - menuRect.height - 6;
  node.style.left = `${left}px`;
  node.style.top = `${Math.max(8, top)}px`;

  node.addEventListener('click', (event) => {
    const button = event.target.closest('[data-index]');
    if (!button) return;
    const item = items[Number(button.dataset.index)];
    closeMenu();
    item?.onSelect?.();
  });

  const outside = (event) => {
    if (!node.contains(event.target) && !anchor.contains(event.target)) closeMenu();
  };
  document.addEventListener('mousedown', outside, true);
  openMenuState = { node, anchor, outside };
}

window.addEventListener('resize', () => closeMenu());
window.addEventListener('blur', () => closeMenu());
