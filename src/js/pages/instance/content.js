// Content tab: installed mods, resource packs, shaders and datapacks, with
// enable/disable, delete, file import (picker or drag & drop) and Modrinth updates.
import * as api from '../../api.js';
import { refreshInstances } from '../../actions.js';
import { setDropHandler } from '../../drop.js';
import { navigate } from '../../router.js';
import { errorText, pickFiles } from '../../tauri.js';
import {
  confirmDialog,
  emptyState,
  errorBlock,
  formatBytes,
  html,
  icon,
  projectTypeBadge,
  projectTypeLabel,
  setHtml,
  spinner,
  toast,
  toastError,
} from '../../ui.js';
import { plural } from './shared.js';

const FILTERS = [
  { value: 'all', label: 'All' },
  { value: 'mod', label: 'Mods' },
  { value: 'resourcepack', label: 'Resource packs' },
  { value: 'shader', label: 'Shaders' },
  { value: 'datapack', label: 'Datapacks' },
];

/** Where imported files go for each filter (null = the backend default, mods). */
const FOLDERS = { mod: 'mods', resourcepack: 'resourcepacks', shader: 'shaderpacks', datapack: 'datapacks' };
/** Discover page type for each filter. */
const DISCOVER = { mod: 'mod', resourcepack: 'resourcepack', shader: 'shader', datapack: 'datapack' };

const isContentFile = (path) => /\.(jar|zip)$/i.test(path);
const displayName = (entry) => entry.title || entry.name.replace(/\.disabled$/i, '');

export function mountContent(pane, ctx) {
  const { id } = ctx;
  let entries = null;
  let loadError = null;
  let filter = 'all';
  let query = '';
  let importing = false;
  /** UpdateInfo[] or null (unknown / offline). */
  let updates = null;
  let checking = false;
  let applyingAll = false;
  let showUpdates = false;
  /** projectIds with an update being applied right now. */
  const applying = new Set();
  /** Paths with a toggle or delete in flight. */
  const pending = new Set();

  setHtml(
    pane,
    html`
      <div class="stack">
        <div data-updates></div>
        <div class="row-between inst-toolbar">
          <div class="row-wrap" style="gap:8px">
            <label class="input-icon" style="width:260px">
              ${icon('search', 'icon-sm')}
              <input class="input" data-search placeholder="Search installed content">
            </label>
            <div class="segmented" data-filters></div>
          </div>
          <div class="row-wrap" style="gap:8px">
            <button class="btn btn-ghost btn-icon btn-sm" data-refresh title="Reload">${icon('refresh', 'icon-sm')}</button>
            <button class="btn btn-outline btn-sm" data-add>${icon('plus', 'icon-sm')} Add files</button>
            <button class="btn btn-primary btn-sm" data-browse>${icon('compass', 'icon-sm')} <span data-browse-label>Browse mods</span></button>
          </div>
        </div>
        <div class="inst-drop-hint small muted" data-drop-hint></div>
        <div data-list></div>
      </div>`,
  );

  const listEl = pane.querySelector('[data-list]');
  const filtersEl = pane.querySelector('[data-filters]');
  const updatesEl = pane.querySelector('[data-updates]');
  const hintEl = pane.querySelector('[data-drop-hint]');
  const browseLabel = pane.querySelector('[data-browse-label]');

  const updateFor = (entry) => (entry.projectId ? updates?.find((u) => u.projectId === entry.projectId) : null);

  // ---------- Drawing ----------

  function drawFilters() {
    const count = (kind) => (entries ?? []).filter((e) => kind === 'all' || e.kind === kind).length;
    setHtml(
      filtersEl,
      FILTERS.map(
        (f) => html`<button data-filter="${f.value}" aria-pressed="${String(f.value === filter)}">${f.label}${entries && html` <span class="faint tiny">${count(f.value)}</span>`}</button>`,
      ),
    );
    browseLabel.textContent = filter === 'all' || filter === 'mod' ? 'Browse mods' : `Browse ${FILTERS.find((f) => f.value === filter).label.toLowerCase()}`;
    const target = filter === 'all' ? 'mods' : FOLDERS[filter];
    hintEl.innerHTML = importing
      ? String(html`${spinner('icon-sm')} Importing…`)
      : String(html`${icon('download', 'icon-sm')} Drop .jar or .zip files anywhere on this page to add them to <code>${target}</code>`);
  }

  function row(entry) {
    const update = updateFor(entry);
    const busy = pending.has(entry.path);
    const fallback = html`<div class="inst-thumb">${icon(entry.kind === 'shader' ? 'sparkles' : entry.kind === 'resourcepack' ? 'image' : 'package', 'icon-sm')}</div>`;
    return html`
      <div class="list-row ${entry.enabled ? '' : 'is-disabled'}" data-path="${entry.path}">
        ${entry.iconUrl
          ? html`<div class="inst-thumb"><img src="${entry.iconUrl}" alt="" loading="lazy" referrerpolicy="no-referrer" data-fallback="${String(fallback)}"></div>`
          : fallback}
        <div class="grow">
          <div class="row" style="gap:8px">
            <strong class="truncate">${displayName(entry)}</strong>
            ${!entry.enabled && html`<span class="badge">Disabled</span>`}
          </div>
          <div class="tiny muted truncate" title="${entry.name}">
            ${entry.title ? `${entry.name} · ` : ''}${formatBytes(entry.size)}${entry.projectId ? '' : ' · local file'}
          </div>
        </div>
        ${filter === 'all' && projectTypeBadge(entry.kind)}
        <div class="row-actions">
          ${update &&
          html`<button class="btn btn-cyan btn-sm" data-update="${update.projectId}" ${applying.has(update.projectId) || applyingAll ? 'disabled' : ''} title="Update to ${update.latestVersionName}">
            ${applying.has(update.projectId) ? spinner('icon-sm') : icon('arrow-up-circle', 'icon-sm')} Update
          </button>`}
          <label class="switch-row" title="${entry.enabled ? 'Disable' : 'Enable'}">
            <input type="checkbox" data-toggle ${entry.enabled ? 'checked' : ''} ${busy ? 'disabled' : ''}>
            <span class="switch"></span>
          </label>
          <button class="btn btn-icon btn-sm danger" data-delete title="Delete" ${busy ? 'disabled' : ''}>${icon('trash', 'icon-sm')}</button>
        </div>
      </div>`;
  }

  function drawList() {
    drawFilters();
    if (loadError) {
      setHtml(listEl, errorBlock(loadError, 'content-retry'));
      listEl.querySelector('#content-retry')?.setAttribute('data-refresh', '');
      return;
    }
    if (!entries) {
      setHtml(listEl, html`<div class="stack-sm">${Array.from({ length: 4 }, () => html`<div class="skeleton" style="height:58px"></div>`)}</div>`);
      return;
    }

    const needle = query.trim().toLowerCase();
    const shown = entries
      .filter((e) => filter === 'all' || e.kind === filter)
      .filter((e) => !needle || displayName(e).toLowerCase().includes(needle) || e.name.toLowerCase().includes(needle))
      .sort((a, b) => displayName(a).localeCompare(displayName(b), undefined, { sensitivity: 'base' }));

    if (!shown.length) {
      const kind = FILTERS.find((f) => f.value === filter).label.toLowerCase();
      setHtml(
        listEl,
        emptyState({
          iconName: 'package',
          title: needle ? 'Nothing matches your search' : filter === 'all' ? 'No content installed' : `No ${kind} installed`,
          description: needle
            ? 'Try another name.'
            : 'Find something on Discover, or drop .jar and .zip files onto this window.',
          actions: needle
            ? ''
            : html`<button class="btn btn-primary btn-sm" data-browse>${icon('compass', 'icon-sm')} Browse</button>
                <button class="btn btn-outline btn-sm" data-add>${icon('plus', 'icon-sm')} Add files</button>`,
        }),
      );
      return;
    }

    if (filter !== 'all') {
      setHtml(listEl, html`<div class="list">${shown.map(row)}</div>`);
      return;
    }

    // Grouped by kind when showing everything.
    const groups = FILTERS.slice(1)
      .map((f) => ({ ...f, items: shown.filter((e) => e.kind === f.value) }))
      .filter((g) => g.items.length);
    setHtml(
      listEl,
      html`<div class="stack">${groups.map(
        (g) => html`
          <section class="stack-sm">
            <div class="inst-group-head">${projectTypeLabel(g.value)}s <span class="faint">${g.items.length}</span></div>
            <div class="list">${g.items.map(row)}</div>
          </section>`,
      )}</div>`,
    );
  }

  function drawUpdates() {
    if (!updates?.length) {
      setHtml(updatesEl, '');
      return;
    }
    const names = updates.slice(0, 3).map((u) => u.title).join(', ');
    setHtml(
      updatesEl,
      html`
        <div class="inst-updates">
          <div class="row" style="gap:12px">
            <span class="inst-updates-icon">${icon('arrow-up-circle')}</span>
            <div class="grow">
              <div class="bold">${plural(updates.length, 'update')} available</div>
              <div class="small muted truncate">${names}${updates.length > 3 ? `, +${updates.length - 3} more` : ''}</div>
            </div>
            <button class="btn btn-ghost btn-sm" data-toggle-updates>${showUpdates ? 'Hide' : 'Show'} ${icon(showUpdates ? 'chevron-up' : 'chevron-down', 'icon-sm')}</button>
            <button class="btn btn-ghost btn-icon btn-sm" data-recheck title="Check again" ${checking ? 'disabled' : ''}>${checking ? spinner('icon-sm') : icon('refresh', 'icon-sm')}</button>
            <button class="btn btn-primary btn-sm" data-update-all ${applyingAll ? 'disabled' : ''}>${applyingAll ? spinner('icon-sm') : icon('arrow-up-circle', 'icon-sm')} Update all</button>
          </div>
          ${showUpdates &&
          html`<div class="list" style="margin-top:12px">${updates.map(
            (u) => html`
              <div class="list-row">
                <div class="inst-thumb">${u.iconUrl ? html`<img src="${u.iconUrl}" alt="" loading="lazy" referrerpolicy="no-referrer" data-fallback="">` : icon('package', 'icon-sm')}</div>
                <div class="grow">
                  <strong class="truncate" style="display:block">${u.title}</strong>
                  <span class="tiny muted">→ ${u.latestVersionName}</span>
                </div>
                ${projectTypeBadge(u.projectType)}
                <button class="btn btn-cyan btn-sm" data-update="${u.projectId}" ${applying.has(u.projectId) || applyingAll ? 'disabled' : ''}>
                  ${applying.has(u.projectId) ? spinner('icon-sm') : icon('arrow-up-circle', 'icon-sm')} Update
                </button>
              </div>`,
          )}</div>`}
        </div>`,
    );
  }

  // ---------- Data ----------

  async function loadContent({ quiet = false } = {}) {
    if (!quiet) {
      entries = null;
      loadError = null;
      drawList();
    }
    try {
      const list = await api.instances.content(id);
      if (!ctx.alive()) return;
      entries = list;
      loadError = null;
    } catch (error) {
      if (!ctx.alive()) return;
      if (quiet && entries) toastError(error, "Couldn't reload the content list");
      else loadError = error;
    }
    drawList();
  }

  async function checkUpdates() {
    checking = true;
    drawUpdates();
    try {
      const list = await api.modrinth.checkUpdates(id);
      if (!ctx.alive()) return;
      updates = list;
    } catch {
      // Offline or rate-limited: stay quiet and hide the banner.
      if (!ctx.alive()) return;
      updates = null;
    }
    checking = false;
    drawUpdates();
    drawList();
  }

  async function applyOne(projectId) {
    const update = updates?.find((u) => u.projectId === projectId);
    if (!update || applying.has(projectId)) return;
    applying.add(projectId);
    drawUpdates();
    drawList();
    try {
      await api.modrinth.applyUpdate(id, projectId, update.latestVersionId);
      toast(`Updated ${update.title}`, { kind: 'success', description: update.latestVersionName });
      if (!ctx.alive()) return;
      updates = updates.filter((u) => u.projectId !== projectId);
    } catch (error) {
      toastError(error, `Couldn't update ${update.title}`);
    }
    applying.delete(projectId);
    if (!ctx.alive()) return;
    drawUpdates();
    await loadContent({ quiet: true });
  }

  async function applyAll() {
    if (!updates?.length || applyingAll) return;
    applyingAll = true;
    drawUpdates();
    drawList();
    const toastId = toast(`Updating ${plural(updates.length, 'project')}…`, { kind: 'loading' });
    try {
      const count = await api.modrinth.updateAll(id);
      toast(`Updated ${plural(count, 'project')}`, { kind: 'success', id: toastId });
    } catch (error) {
      toast("Couldn't finish updating", { kind: 'error', id: toastId, description: errorText(error) });
    }
    applyingAll = false;
    if (!ctx.alive()) return;
    refreshInstances();
    await loadContent({ quiet: true });
    checkUpdates();
  }

  async function importPaths(paths) {
    const files = paths.filter(isContentFile);
    if (!files.length) {
      toast('Only .jar and .zip files can be added here.', { kind: 'warn' });
      return;
    }
    importing = true;
    drawFilters();
    try {
      const count = await api.instances.importFiles(id, files, FOLDERS[filter] ?? null);
      const skipped = files.length - count;
      toast(count ? `Added ${plural(count, 'file')}` : 'Nothing was added', {
        kind: count ? 'success' : 'warn',
        description: skipped ? `${plural(skipped, 'file')} skipped (already installed or not a .jar/.zip).` : undefined,
      });
      refreshInstances();
    } catch (error) {
      toastError(error, "Couldn't add those files");
    }
    importing = false;
    if (!ctx.alive()) return;
    await loadContent({ quiet: true });
  }

  async function toggle(entry, enabled) {
    pending.add(entry.path);
    drawList();
    try {
      const newPath = await api.instances.toggleContent(id, entry.path, enabled);
      pending.delete(entry.path);
      entry.path = newPath;
      entry.name = newPath.split(/[\\/]/).pop();
      entry.enabled = enabled;
    } catch (error) {
      pending.delete(entry.path);
      toastError(error, `Couldn't ${enabled ? 'enable' : 'disable'} ${displayName(entry)}`);
    }
    if (ctx.alive()) drawList();
  }

  async function remove(entry) {
    const ok = await confirmDialog({
      title: `Delete ${displayName(entry)}?`,
      message: 'The file is removed from this instance for good.',
      details: entry.name,
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!ok || !ctx.alive()) return;
    pending.add(entry.path);
    drawList();
    try {
      await api.instances.deleteContent(id, entry.path);
      entries = entries.filter((e) => e !== entry);
      toast(`Deleted ${displayName(entry)}`, { kind: 'success' });
      refreshInstances();
    } catch (error) {
      toastError(error, "Couldn't delete that file");
    }
    pending.delete(entry.path);
    if (ctx.alive()) drawList();
  }

  // ---------- Events ----------

  const entryOf = (node) => {
    const path = node.closest('[data-path]')?.dataset.path;
    return path == null ? null : entries?.find((e) => e.path === path);
  };

  pane.addEventListener('click', async (event) => {
    const t = event.target;
    const filterBtn = t.closest('[data-filter]');
    if (filterBtn) {
      filter = filterBtn.dataset.filter;
      return drawList();
    }
    if (t.closest('[data-refresh]')) return loadContent();
    if (t.closest('[data-browse]')) {
      return navigate(`/discover/${DISCOVER[filter] ?? 'mod'}?instance=${encodeURIComponent(id)}`);
    }
    if (t.closest('[data-add]')) {
      const paths = await pickFiles({ title: 'Add mods or packs', extensions: ['jar', 'zip'] });
      if (paths.length && ctx.alive()) importPaths(paths);
      return;
    }
    if (t.closest('[data-toggle-updates]')) {
      showUpdates = !showUpdates;
      return drawUpdates();
    }
    if (t.closest('[data-recheck]')) return checkUpdates();
    if (t.closest('[data-update-all]')) return applyAll();
    const updateBtn = t.closest('[data-update]');
    if (updateBtn) return applyOne(updateBtn.dataset.update);
    if (t.closest('[data-delete]')) {
      const entry = entryOf(t);
      if (entry) remove(entry);
    }
  });

  pane.addEventListener('change', (event) => {
    if (!event.target.matches('[data-toggle]')) return;
    const entry = entryOf(event.target);
    if (entry) toggle(entry, event.target.checked);
  });

  pane.querySelector('[data-search]').addEventListener('input', (event) => {
    query = event.target.value;
    drawList();
  });

  const offDrop = setDropHandler({
    label: 'Drop .jar or .zip files to add them to this instance',
    accepts: (paths) => paths.some(isContentFile),
    onDrop: (paths) => importPaths(paths),
  });

  drawUpdates();
  loadContent();
  checkUpdates();

  return () => offDrop();
}
