// Screenshots tab: the PNGs in the instance's `screenshots/` folder, a
// full-size viewer with previous/next, and delete.
import * as api from '../../api.js';
import { fileSrc } from '../../tauri.js';
import {
  confirmDialog,
  emptyState,
  errorBlock,
  formatBytes,
  html,
  icon,
  loadingBlock,
  openModal,
  relativeTime,
  setHtml,
  spinner,
  toast,
  toastError,
} from '../../ui.js';
import { plural, reveal } from './shared.js';

export function mountScreenshots(pane, ctx) {
  const { id } = ctx;
  let file = null;
  let loadError = null;
  let loading = true;
  let viewer = null;

  function draw() {
    if (loading) return setHtml(pane, loadingBlock('Looking for screenshots…'));
    if (loadError || !file) {
      setHtml(pane, errorBlock(loadError ?? 'No data', 'shots-retry'));
      pane.querySelector('#shots-retry')?.setAttribute('data-reload', '');
      return;
    }
    const shots = file.screenshots;
    setHtml(
      pane,
      html`
        <div class="stack">
          <div class="row-between">
            <span class="small muted">${shots.length ? `${plural(shots.length, 'screenshot')}, newest first` : 'No screenshots yet'}</span>
            <div class="row" style="gap:6px">
              <button class="btn btn-ghost btn-sm" data-reload>${icon('refresh', 'icon-sm')} Refresh</button>
              <button class="btn btn-ghost btn-sm" data-open-folder ${file.exists ? '' : 'disabled'}>${icon('folder', 'icon-sm')} Open folder</button>
            </div>
          </div>
          ${shots.length === 0
            ? emptyState({
                iconName: 'image',
                title: 'No screenshots yet',
                description: 'Press F2 in game and the shot lands here, ready to view or clear out.',
              })
            : html`<div class="shot-grid">${shots.map(
                (shot, index) => html`
                  <div class="shot-tile" data-index="${index}" title="${shot.name} · ${formatBytes(shot.size)}">
                    <button class="shot-open" data-view>
                      <img src="${fileSrc(shot.path)}" alt="${shot.name}" loading="lazy" decoding="async">
                    </button>
                    <div class="shot-foot">
                      <span class="tiny muted truncate">${shot.taken ? relativeTime(shot.taken) : shot.name}</span>
                      <button class="btn btn-ghost btn-icon btn-sm danger" data-delete-shot title="Delete">${icon('trash', 'icon-sm')}</button>
                    </div>
                  </div>`,
              )}</div>`}
        </div>`,
    );
  }

  async function load(silent = false) {
    if (!silent) {
      loading = true;
      draw();
    }
    try {
      const next = await api.screenshots.list(id);
      if (!ctx.alive()) return;
      file = next;
      loadError = null;
    } catch (error) {
      if (!ctx.alive()) return;
      loadError = error;
    }
    loading = false;
    draw();
  }

  async function remove(shot) {
    try {
      const next = await api.screenshots.remove(id, shot.name);
      toast(`Deleted ${shot.name}`, { kind: 'success' });
      if (!ctx.alive()) return true;
      file = next;
      draw();
      return true;
    } catch (error) {
      toastError(error, "Couldn't delete the screenshot");
      return false;
    }
  }

  // ---------- Viewer ----------

  function openViewer(startIndex) {
    viewer?.close();
    let index = startIndex;
    let confirming = false;
    let busy = false;

    const modal = openModal({
      title: ' ',
      size: 'xl',
      body: html`<div data-viewer></div>`,
      onClose: () => {
        document.removeEventListener('keydown', onKey);
        if (viewer === modal) viewer = null;
      },
    });
    viewer = modal;
    const slot = modal.body.querySelector('[data-viewer]');
    const titleEl = modal.root.querySelector('.modal-head h2');

    const drawViewer = () => {
      const shots = file?.screenshots ?? [];
      const shot = shots[index];
      if (!shot) return modal.close();
      titleEl.textContent = shot.name;
      titleEl.classList.add('mono', 'truncate');
      setHtml(
        slot,
        html`
          <div class="shot-stage">
            <img src="${fileSrc(shot.path)}" alt="${shot.name}">
            <button class="btn btn-icon shot-nav shot-prev" data-step="-1" title="Previous" ${index === 0 ? 'disabled' : ''}>${icon('chevron-left')}</button>
            <button class="btn btn-icon shot-nav shot-next" data-step="1" title="Next" ${index >= shots.length - 1 ? 'disabled' : ''}>${icon('chevron-right')}</button>
          </div>
          <div class="row-between" style="margin-top:12px">
            <span class="small muted">${formatBytes(shot.size)} · ${shot.taken ? relativeTime(shot.taken) : 'unknown date'} · ${index + 1} of ${shots.length}</span>
            ${confirming
              ? html`<div class="row" style="gap:8px">
                  <span class="small muted">Delete this screenshot? The PNG goes for good.</span>
                  <button class="btn btn-ghost btn-sm" data-keep>Keep it</button>
                  <button class="btn btn-danger btn-sm" data-confirm-delete ${busy ? 'disabled' : ''}>${busy ? spinner('icon-sm') : icon('trash', 'icon-sm')} Delete</button>
                </div>`
              : html`<div class="row" style="gap:8px">
                  <button class="btn btn-ghost btn-sm" data-folder>${icon('folder', 'icon-sm')} Open folder</button>
                  <button class="btn btn-outline btn-sm danger-text" data-ask-delete>${icon('trash', 'icon-sm')} Delete</button>
                </div>`}
          </div>`,
      );
    };

    const step = (by) => {
      const next = index + by;
      if (next >= 0 && next < (file?.screenshots.length ?? 0)) {
        index = next;
        confirming = false;
        drawViewer();
      }
    };

    function onKey(event) {
      if (event.key === 'ArrowLeft') step(-1);
      if (event.key === 'ArrowRight') step(1);
    }
    document.addEventListener('keydown', onKey);

    slot.addEventListener('click', async (event) => {
      const t = event.target;
      const stepBtn = t.closest('[data-step]');
      if (stepBtn) return step(Number(stepBtn.dataset.step));
      if (t.closest('[data-folder]')) return reveal(file?.path);
      if (t.closest('[data-ask-delete]')) {
        confirming = true;
        return drawViewer();
      }
      if (t.closest('[data-keep]')) {
        confirming = false;
        return drawViewer();
      }
      if (t.closest('[data-confirm-delete]') && !busy) {
        const shot = file?.screenshots[index];
        if (!shot) return;
        busy = true;
        drawViewer();
        const ok = await remove(shot);
        busy = false;
        confirming = false;
        if (ok) modal.close();
        else if (!modal.closed) drawViewer();
      }
    });

    drawViewer();
  }

  pane.addEventListener('click', async (event) => {
    const t = event.target;
    if (t.closest('[data-reload]')) return load(true);
    if (t.closest('[data-open-folder]')) return reveal(file?.path);
    const tile = t.closest('[data-index]');
    if (!tile || !file) return;
    const index = Number(tile.dataset.index);
    if (t.closest('[data-view]')) return openViewer(index);
    if (t.closest('[data-delete-shot]')) {
      const shot = file.screenshots[index];
      const ok = await confirmDialog({
        title: 'Delete this screenshot?',
        message: 'The PNG goes for good — nothing keeps a copy.',
        details: shot.name,
        confirmLabel: 'Delete',
        danger: true,
      });
      if (ok && ctx.alive()) remove(shot);
    }
  });

  load();
  return () => viewer?.close();
}
