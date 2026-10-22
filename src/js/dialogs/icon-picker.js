// Instance icon chooser: built-in emoji or a custom image file.
import { pickFile } from '../tauri.js';
import { closeMenu, html, icon } from '../ui.js';

const PRESETS = [
  '🌲', '⛏️', '⚔️', '🛡️', '🏹', '🧪', '🔮', '🔥', '❄️', '⚡', '🌊', '🌪️', '🌙', '☀️', '🌈', '🍀',
  '🍄', '🐝', '🐉', '🦄', '🎃', '💀', '🤖', '👾', '🚀', '🏰', '🧱', '💰', '📚', '🎵', '🌱', '🕹️',
];

/**
 * Show the picker next to `anchor`. Resolves with the new icon value (an emoji,
 * an image path, or null for "automatic"), or `undefined` when dismissed.
 * Image files are copied into the instance folder by the backend on save.
 */
export function pickIcon(anchor, current) {
  closeMenu();
  return new Promise((resolve) => {
    const node = document.createElement('div');
    node.className = 'menu';
    node.style.width = '300px';
    node.style.padding = '12px';
    node.innerHTML = String(html`
      <div class="stack-sm">
        <div class="small bold">Emoji</div>
        <div style="display:grid;grid-template-columns:repeat(8,1fr);gap:3px">
          ${PRESETS.map(
            (emoji) =>
              html`<button class="btn btn-ghost btn-sm" data-emoji="${emoji}" style="padding:4px;font-size:18px;${current === emoji ? 'background:var(--panel-3)' : ''}">${emoji}</button>`,
          )}
        </div>
        <hr class="divider">
        <div class="row-between">
          <div><div class="small bold">Custom image</div><div class="tiny muted">PNG, JPG, WebP…</div></div>
          <button class="btn btn-outline btn-sm" data-browse>${icon('folder')} Browse…</button>
        </div>
        ${current && html`<button class="btn btn-ghost btn-sm" data-reset>${icon('rotate-ccw', 'icon-sm')} Back to automatic</button>`}
      </div>`);
    document.body.appendChild(node);

    const rect = anchor.getBoundingClientRect();
    const box = node.getBoundingClientRect();
    node.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - box.width - 8))}px`;
    const below = rect.bottom + 6;
    node.style.top = `${below + box.height > window.innerHeight - 8 ? Math.max(8, rect.top - box.height - 6) : below}px`;

    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      node.remove();
      document.removeEventListener('mousedown', outside, true);
      document.removeEventListener('keydown', onKey, true);
      resolve(value);
    };
    const outside = (event) => !node.contains(event.target) && finish(undefined);
    const onKey = (event) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        finish(undefined);
      }
    };
    document.addEventListener('mousedown', outside, true);
    document.addEventListener('keydown', onKey, true);

    node.addEventListener('click', async (event) => {
      const emoji = event.target.closest('[data-emoji]');
      if (emoji) return finish(emoji.dataset.emoji);
      if (event.target.closest('[data-reset]')) return finish(null);
      if (event.target.closest('[data-browse]')) {
        document.removeEventListener('mousedown', outside, true);
        const path = await pickFile({
          title: 'Choose an icon image',
          extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'ico'],
        });
        finish(path ?? undefined);
      }
    });
  });
}
