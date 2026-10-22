// Renders Modrinth / CurseForge project bodies and changelogs.
//
// Bodies are Markdown with raw HTML mixed in (authors ship `<center>`, `<img>`
// and tables directly), so the HTML is parsed rather than escaped. That is only
// safe because DOMPurify runs *after* the parse, with a GitHub-style allowlist:
// no scripts, event handlers, `javascript:` URLs, inline `style`, classes,
// `data-*` attributes, forms or frames. Nothing reaches the DOM unsanitised.
//
// The parser (vendor/marked.umd.js) and sanitiser (vendor/purify.min.js) are
// loaded on first use.
import * as api from './api.js';
import { toastError } from './ui.js';

const scripts = new Map();

/** Inject a classic <script> once and resolve when it has loaded. */
function loadScript(src) {
  if (!scripts.has(src)) {
    scripts.set(
      src,
      new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.async = true;
        script.onload = () => resolve();
        script.onerror = () => {
          scripts.delete(src);
          script.remove();
          reject(new Error(`Couldn't load ${src}`));
        };
        document.head.appendChild(script);
      }),
    );
  }
  return scripts.get(src);
}

let ready = null;

/** Load both libraries and configure them (once). Resolves to { parser, purify }. */
function libraries() {
  ready ??= Promise.all([loadScript('vendor/marked.umd.js'), loadScript('vendor/purify.min.js')])
    .then(() => {
      const { marked, DOMPurify } = window;
      if (!marked || !DOMPurify) throw new Error('The Markdown renderer failed to initialise.');
      const parser = marked.Marked ? new marked.Marked({ gfm: true, breaks: false }) : marked;
      configurePurify(DOMPurify);
      return { parser, purify: DOMPurify };
    })
    .catch((error) => {
      ready = null;
      throw error;
    });
  return ready;
}

const PURIFY_CONFIG = {
  USE_PROFILES: { html: true },
  ADD_TAGS: ['center'],
  FORBID_TAGS: ['style', 'form', 'input', 'button', 'textarea', 'select', 'option', 'iframe', 'object', 'embed', 'video', 'audio', 'source', 'svg', 'math'],
  FORBID_ATTR: ['style', 'class', 'srcset', 'sizes', 'target', 'ping', 'formaction'],
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: true,
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#|data:image\/(?:png|gif|jpe?g|webp);)/i,
};

const ID_PREFIX = 'user-content-';

function configurePurify(purify) {
  purify.addHook('afterSanitizeAttributes', (node) => {
    // Prefix ids and names so a body can't clobber the launcher's own
    // elements; in-page links are resolved against the prefix on click.
    for (const attr of ['id', 'name']) {
      const value = node.getAttribute?.(attr);
      if (value && !value.startsWith(ID_PREFIX)) node.setAttribute(attr, ID_PREFIX + value);
    }
    if (node.tagName === 'A') {
      const href = node.getAttribute('href');
      if (href && !href.startsWith('#')) node.setAttribute('rel', 'noopener noreferrer');
    }
    if (node.tagName === 'IMG') {
      node.setAttribute('loading', 'lazy');
      node.setAttribute('referrerpolicy', 'no-referrer');
      node.setAttribute('decoding', 'async');
    }
  });
}

/** Markdown (with embedded HTML) to sanitised HTML. */
export async function renderMarkdown(source) {
  const { parser, purify } = await libraries();
  const parsed = parser.parse(String(source ?? ''), { async: false });
  return purify.sanitize(parsed, PURIFY_CONFIG);
}

/**
 * Render `source` into `container` (which gets the `markdown` class), wire
 * links to open in the system browser, wrap wide tables so they scroll, and
 * hide images the CSP or network refused. Resolves once rendered; resolves
 * to false when `isStale()` reports the container is no longer wanted.
 */
export async function mountMarkdown(container, source, { isStale } = {}) {
  const safe = await renderMarkdown(source);
  if (isStale?.() || !container.isConnected) return false;
  container.classList.add('markdown');
  container.innerHTML = safe;

  for (const table of container.querySelectorAll('table')) {
    const wrap = document.createElement('div');
    wrap.className = 'md-table';
    table.replaceWith(wrap);
    wrap.appendChild(table);
  }
  for (const img of container.querySelectorAll('img')) {
    const hide = () => img.classList.add('md-broken');
    img.addEventListener('error', hide, { once: true });
    if (img.complete && img.naturalWidth === 0 && img.getAttribute('src')) hide();
  }

  if (!container.dataset.mdWired) {
    container.dataset.mdWired = '1';
    container.addEventListener('click', onLinkClick);
  }
  return true;
}

function onLinkClick(event) {
  const link = event.target.closest('a[href]');
  if (!link || !event.currentTarget.contains(link)) return;
  const href = link.getAttribute('href');
  event.preventDefault();
  if (href.startsWith('#')) {
    // Tables of contents keep working: jump to the (prefixed) anchor.
    const name = decodeURIComponent(href.slice(1));
    if (!name) return;
    const root = event.currentTarget;
    const target =
      root.querySelector(`#${CSS.escape(ID_PREFIX + name)}`) ??
      root.querySelector(`[name="${CSS.escape(ID_PREFIX + name)}"]`) ??
      [...root.querySelectorAll('h1, h2, h3, h4, h5, h6')].find((h) => slugify(h.textContent) === slugify(name));
    target?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    return;
  }
  if (/^(https?:|mailto:)/i.test(href)) api.system.openUrl(href).catch(toastError);
}

const slugify = (text) =>
  String(text ?? '')
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .replace(/\s+/g, '-');
