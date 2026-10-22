// "Import a modpack": choose a local .mrpack or CurseForge .zip, preview it,
// optionally rename it, and create the instance.
import * as api from '../api.js';
import { refreshInstances } from '../actions.js';
import { navigate } from '../router.js';
import { errorText, pickFile } from '../tauri.js';
import { formatBytes, html, icon, loaderBadge, openModal, setHtml, spinner, toast } from '../ui.js';

/** Open the dialog; with `path` (e.g. a dropped file) it skips the picker. */
export function openImportModpackDialog(path) {
  let packPath = '';
  let preview = null;
  let loading = false;
  let busy = false;
  let error = '';

  const modal = openModal({
    title: 'Import a modpack',
    description: html`Open a <code>.mrpack</code> from Modrinth or a CurseForge modpack <code>.zip</code>.`,
    body: html`<div class="stack" data-pack></div>`,
    footer: html`
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" data-import hidden></button>`,
  });
  const slot = modal.body.querySelector('[data-pack]');
  const importBtn = modal.foot.querySelector('[data-import]');

  function draw() {
    if (modal.closed) return;
    importBtn.hidden = !preview || loading;
    importBtn.disabled = busy;
    setHtml(importBtn, html`${busy ? spinner() : icon('package')} ${busy ? 'Importing…' : 'Import'}`);

    if (!preview || loading) {
      setHtml(
        slot,
        html`
          <button class="pack-chooser" data-choose ${loading ? 'disabled' : ''}>
            ${loading ? spinner() : icon('folder', 'icon-lg')}
            <span class="bold">${loading ? 'Reading pack…' : 'Choose a modpack file'}</span>
          </button>
          <p class="form-error selectable">${error}</p>`,
      );
      return;
    }

    const name = slot.querySelector('[data-name]')?.value ?? preview.name;
    // CurseForge manifests carry no sizes (totalSize 0), so none is shown then.
    setHtml(
      slot,
      html`
        <div class="row pack-file">
          <span class="pack-file-icon">${icon('archive', 'icon-lg')}</span>
          <div class="grow">
            <div class="truncate bold">${packPath.split(/[\\/]/).pop()}</div>
            <div class="truncate small muted selectable" title="${packPath}">${packPath}</div>
          </div>
          <button class="btn btn-ghost btn-sm" data-choose ${busy ? 'disabled' : ''}>Choose another</button>
        </div>
        <div class="row-wrap">
          <span class="badge badge-mono">${preview.gameVersion}</span>
          ${loaderBadge(preview.loader)}
          ${preview.loaderVersion && html`<span class="badge badge-mono">${preview.loaderVersion}</span>`}
          <span class="badge">${preview.fileCount} files</span>
          ${preview.totalSize > 0 && html`<span class="badge">${formatBytes(preview.totalSize)}</span>`}
        </div>
        ${preview.summary && html`<p class="muted" style="line-height:1.5">${preview.summary}</p>`}
        <label class="field">
          <span>Instance name</span>
          <input class="input" data-name maxlength="64" value="${name}" placeholder="${preview.name}" ${busy ? 'disabled' : ''}>
        </label>
        <p class="form-error selectable">${error}</p>`,
    );
  }

  async function load(chosen) {
    loading = true;
    error = '';
    draw();
    try {
      const result = await api.modpacks.preview(chosen);
      if (modal.closed) return;
      packPath = chosen;
      preview = result;
      // A new pack gets its own name, not the previous pack's.
      slot.querySelector('[data-name]')?.remove();
    } catch (cause) {
      error = errorText(cause);
    } finally {
      loading = false;
    }
    draw();
    slot.querySelector('[data-name]')?.focus();
  }

  async function choose() {
    if (loading || busy) return;
    const chosen = await pickFile({ title: 'Choose a modpack file', extensions: ['mrpack', 'zip'] });
    if (chosen && !modal.closed) load(chosen);
  }

  async function runImport() {
    if (!packPath || busy) return;
    const name = slot.querySelector('[data-name]')?.value.trim();
    busy = true;
    error = '';
    modal.setDismissable(false);
    draw();
    try {
      const instance = await api.modpacks.import(packPath, name || undefined);
      await refreshInstances();
      toast(`Imported “${instance.name}”`, { kind: 'success' });
      modal.setDismissable(true);
      modal.close();
      navigate(`/instance/${instance.id}`);
    } catch (cause) {
      error = errorText(cause);
      busy = false;
      modal.setDismissable(true);
      draw();
    }
  }

  slot.addEventListener('click', (event) => {
    if (event.target.closest('[data-choose]')) choose();
  });
  slot.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && event.target.matches('[data-name]')) runImport();
  });
  importBtn.addEventListener('click', runImport);

  if (path) load(path);
  else draw();
}
