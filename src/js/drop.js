// Window-wide file drag & drop. Pages register what a drop means while they
// are shown; the overlay only appears when the current handler accepts the files.
import { onFileDrop } from './tauri.js';
import { el, html, icon } from './ui.js';

let handler = null;
let overlay = null;

/**
 * Register the current page's drop handler:
 *   { label, accepts(paths) -> bool, onDrop(paths) }
 * Returns a function that unregisters it (call from the page cleanup).
 */
export function setDropHandler(next) {
  handler = next;
  return () => {
    if (handler === next) handler = null;
    hideOverlay();
  };
}

function showOverlay(label) {
  if (overlay) return;
  overlay = el(html`<div class="drop-overlay"><div>
    <div style="color:var(--primary);margin-bottom:10px">${icon('download', 'icon-lg')}</div>
    <div class="bold">${label}</div>
  </div></div>`);
  document.body.appendChild(overlay);
}

function hideOverlay() {
  overlay?.remove();
  overlay = null;
}

export function initDrop() {
  let pending = [];
  onFileDrop(({ type, paths }) => {
    if (type === 'enter') pending = paths ?? [];
    if (type === 'enter' || type === 'over') {
      if (handler && (pending.length === 0 || handler.accepts(pending))) showOverlay(handler.label);
      return;
    }
    if (type === 'leave') {
      hideOverlay();
      return;
    }
    if (type === 'drop') {
      hideOverlay();
      if (handler && paths?.length && handler.accepts(paths)) handler.onDrop(paths);
    }
  });
}
