// "Import from another launcher": scan Prism/MultiMC, CurseForge, the Modrinth
// App and the vanilla launcher (or a folder picked by hand), let the player
// tick and adjust instances, then copy them into new MineArcade instances.
// The scan only reads metadata; nothing in the other launcher is changed.
import * as api from '../api.js';
import { refreshInstances } from '../actions.js';
import { store } from '../store.js';
import { errorText, pickDirectory } from '../tauri.js';
import { pickIcon } from './icon-picker.js';
import {
  el,
  formatBytes,
  html,
  icon,
  instanceIcon,
  LOADERS,
  loaderLabel,
  openModal,
  relativeTime,
  setHtml,
  spinner,
  toast,
  wireImages,
} from '../ui.js';

/** Friendly names for each launcher the scan knows about. */
const SOURCE_LABELS = {
  prism: 'Prism Launcher',
  multimc: 'MultiMC',
  curseforge: 'CurseForge',
  modrinth: 'Modrinth App',
  minecraft: 'Minecraft Launcher',
  folder: 'Chosen folder',
};

const plural = (count, word) => `${count} ${word}${count === 1 ? '' : 's'}`;

export function openImportLauncherDialog() {
  /** ImportCandidate[] in discovery order. */
  const candidates = [];
  /** Editable state per candidate id: { checked, name, gameVersion, loader, loaderVersion, icon, expanded } */
  const rows = new Map();
  /** Loader builds per `${loader}|${gameVersion}`: { loading, list, failed } */
  const loaderBuilds = new Map();
  let gameVersions = [];
  let showSnapshots = false;
  let scanning = true;
  let browsing = false;
  let importing = false;
  let sourceNotes = [];
  let offProgress = null;

  const modal = openModal({
    title: 'Import from another launcher',
    description:
      'MineArcade copies the game folders you already have into new instances. Nothing in the original launcher is moved, renamed or changed.',
    size: 'xl',
    body: html`
      <div class="import-notes" data-notes hidden></div>
      <div data-list></div>
      <div class="row-between small muted">
        <button class="btn btn-ghost btn-sm" data-browse></button>
        <span data-summary></span>
      </div>
      <div class="stack-sm" data-progress hidden></div>`,
    footer: html`
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" data-import disabled></button>`,
    onClose: () => offProgress?.(),
  });

  const body = modal.body;
  const listEl = body.querySelector('[data-list]');
  const notesEl = body.querySelector('[data-notes]');
  const summaryEl = body.querySelector('[data-summary]');
  const browseBtn = body.querySelector('[data-browse]');
  const progressEl = body.querySelector('[data-progress]');
  const importBtn = modal.foot.querySelector('[data-import]');

  // ----- derived state -----

  /** Everything ticked, ready for the backend. */
  function requests() {
    const list = [];
    for (const candidate of candidates) {
      const row = rows.get(candidate.id);
      if (!row?.checked) continue;
      list.push({
        source: candidate.kind,
        name: row.name.trim() || candidate.name,
        path: candidate.path,
        gameVersion: row.gameVersion,
        loader: row.loader,
        loaderVersion: row.loader === 'vanilla' ? null : row.loaderVersion,
        // The player's pick wins; it starts as the launcher's own icon.
        icon: row.icon,
      });
    }
    return list;
  }

  // ----- rendering -----

  function gameVersionOptions(value) {
    const ids = gameVersions.filter((v) => showSnapshots || v.versionType === 'release').map((v) => v.id);
    // Keep the candidate's own value even when the manifest doesn't list it.
    if (value && !ids.includes(value)) ids.unshift(value);
    return html`${!value && html`<option value="" selected>Pick a version</option>`}${ids.map(
      (id) => html`<option value="${id}" ${id === value ? 'selected' : ''}>${id}</option>`,
    )}`;
  }

  function loaderBuildControl(row) {
    if (row.loader === 'vanilla') return html`<div class="import-no-build">No loader build</div>`;
    const builds = row.gameVersion ? loaderBuilds.get(`${row.loader}|${row.gameVersion}`) : null;
    if (!builds || builds.loading) {
      return html`<select class="input" disabled><option>${row.gameVersion ? 'Loading builds…' : 'Pick a version first'}</option></select>`;
    }
    if (!builds.list.length) {
      // The backend falls back to the latest stable build.
      return html`<select class="input" disabled><option>Latest stable</option></select>`;
    }
    return html`<select class="input" data-loader-version>
      ${builds.list.map(
        (build) => html`<option value="${build.version}" ${build.version === row.loaderVersion ? 'selected' : ''}>${build.version}${build.stable ? '' : ' (beta)'}</option>`,
      )}
    </select>`;
  }

  function rowMarkup(candidate) {
    const row = rows.get(candidate.id);
    const name = row.name || candidate.name;
    const modified = candidate.modified != null ? relativeTime(new Date(candidate.modified).toISOString()) : null;
    return html`
      <div class="import-row ${row.checked ? '' : 'is-unchecked'}" data-row="${candidate.id}">
        <div class="row">
          <input type="checkbox" data-check ${row.checked ? 'checked' : ''} aria-label="Import ${candidate.name}">
          ${instanceIcon({ icon: row.icon, name, id: candidate.id }, 36)}
          <div class="grow">
            <div class="truncate bold" data-title>${name}</div>
            <div class="row-wrap small muted import-meta">
              ${row.gameVersion
                ? html`<span class="badge badge-mono">${row.gameVersion}</span>`
                : html`<span class="badge badge-warning">version needed</span>`}
              <span class="badge">${loaderLabel(row.loader)}</span>
              <span>${plural(candidate.mods, 'mod')}</span>
              <span>· ${formatBytes(candidate.size)}</span>
              ${modified && html`<span>· changed ${modified}</span>`}
              ${candidate.note && html`<span class="truncate">· ${candidate.note}</span>`}
            </div>
          </div>
          <button class="btn btn-ghost btn-sm" data-expand aria-expanded="${row.expanded}">
            ${icon(row.expanded ? 'chevron-down' : 'chevron-right', 'icon-sm')} Edit
          </button>
        </div>
        ${row.expanded &&
        html`
          <hr class="divider">
          <div class="import-edit">
            <div class="field"><span class="tiny muted">Icon</span>
              <button class="btn btn-outline btn-sm" data-pick-icon>${icon('image', 'icon-sm')} Change</button></div>
            <label class="field import-edit-name"><span class="tiny muted">Name</span>
              <input class="input" data-name maxlength="64" value="${row.name}" spellcheck="false"></label>
            <label class="field"><span class="tiny muted">Minecraft</span>
              <select class="input" data-game-version>${gameVersionOptions(row.gameVersion)}</select></label>
            <label class="field"><span class="tiny muted">Loader</span>
              <select class="input" data-loader>
                ${LOADERS.map((kind) => html`<option value="${kind}" ${kind === row.loader ? 'selected' : ''}>${loaderLabel(kind)}</option>`)}
              </select></label>
            <div class="field"><span class="tiny muted">Loader build</span>${loaderBuildControl(row)}</div>
            ${!showSnapshots && html`<button class="btn btn-ghost btn-sm" data-snapshots>${icon('sparkles', 'icon-sm')} Show snapshots</button>`}
          </div>`}
      </div>`;
  }

  function renderList() {
    if (modal.closed) return;
    if (notesEl) {
      notesEl.hidden = !sourceNotes.length;
      setHtml(notesEl, html`${sourceNotes.map((note) => html`<p class="truncate" title="${note}">${note}</p>`)}`);
    }
    if (scanning) {
      setHtml(
        listEl,
        html`<div class="stack-sm"><div class="skeleton" style="height:16px;width:160px"></div>
          <div class="skeleton" style="height:76px"></div><div class="skeleton" style="height:76px"></div></div>`,
      );
    } else if (!candidates.length) {
      setHtml(
        listEl,
        html`<div class="empty">
          <div class="empty-icon">${icon('package', 'icon-lg')}</div>
          <h3>No instances were detected</h3>
          <p>Pick a launcher’s data folder (or one instance folder) and MineArcade will read it from there.</p>
        </div>`,
      );
    } else {
      // Grouped by launcher so the list reads "Prism, then CurseForge…".
      const groups = new Map();
      for (const candidate of candidates) {
        if (!groups.has(candidate.kind)) groups.set(candidate.kind, []);
        groups.get(candidate.kind).push(candidate);
      }
      setHtml(
        listEl,
        html`<div class="stack-lg">
          ${[...groups].map(
            ([kind, group]) => html`
              <div class="stack-sm">
                <div class="row"><span class="bold">${SOURCE_LABELS[kind] ?? group[0].source}</span>
                  <span class="badge">${plural(group.length, 'instance')}</span></div>
                ${group.map(rowMarkup)}
              </div>`,
          )}
        </div>`,
      );
    }
    renderBar();
  }

  function renderRow(id) {
    const node = listEl.querySelector(`[data-row="${CSS.escape(id)}"]`);
    const candidate = candidates.find((c) => c.id === id);
    if (!node || !candidate || modal.closed) return;
    const fresh = el(rowMarkup(candidate));
    node.replaceWith(fresh);
    wireImages(fresh);
    renderBar();
  }

  function renderBar() {
    const list = requests();
    const missingVersion = list.some((request) => !request.gameVersion);
    const totalBytes = candidates.reduce((sum, c) => (rows.get(c.id)?.checked ? sum + c.size : sum), 0);
    summaryEl.textContent = `${list.length} selected · ${formatBytes(totalBytes)}${missingVersion ? ' · pick a version for every instance' : ''}`;
    setHtml(browseBtn, html`${browsing ? spinner('icon-sm') : icon('folder-input', 'icon-sm')} Browse for a folder…`);
    browseBtn.disabled = browsing || importing;
    setHtml(importBtn, html`${importing ? spinner() : icon('package')} Import ${plural(list.length, 'instance')}`);
    importBtn.disabled = !list.length || missingVersion || importing;
  }

  function renderProgress() {
    const progress = store.state.progress;
    progressEl.hidden = !importing || !progress || progress.finished;
    if (progressEl.hidden) return;
    const percent = progress.total > 0 ? Math.min(100, Math.round((progress.current / progress.total) * 100)) : 0;
    setHtml(
      progressEl,
      html`<div class="row-between small"><span class="truncate">${progress.label}</span><span class="muted">${percent}%</span></div>
        <div class="progress-track" style="height:6px"><div class="progress-fill" style="width:${percent}%"></div></div>`,
    );
  }

  // ----- data -----

  /** Merge newly discovered candidates into the list. */
  function absorb(found) {
    const seen = new Set(candidates.map((c) => c.id));
    for (const candidate of found) {
      if (seen.has(candidate.id)) continue;
      seen.add(candidate.id);
      candidates.push(candidate);
      rows.set(candidate.id, {
        checked: true,
        name: candidate.name,
        gameVersion: candidate.gameVersion ?? '',
        loader: candidate.loader ?? 'vanilla',
        loaderVersion: candidate.loaderVersion,
        icon: candidate.icon,
        // Metadata holes mean "this row needs a decision": open it up.
        expanded: !candidate.gameVersion || !candidate.loader,
      });
    }
    for (const candidate of found) if (rows.get(candidate.id)?.expanded) loadBuilds(candidate.id);
  }

  /** Fetch the loader builds for a row's loader/version pair, preselecting one. */
  async function loadBuilds(id) {
    const row = rows.get(id);
    if (!row || row.loader === 'vanilla' || !row.gameVersion) return;
    const key = `${row.loader}|${row.gameVersion}`;
    const cached = loaderBuilds.get(key);
    if (cached && !cached.loading) return preselect(row, cached.list);
    if (cached) return;
    loaderBuilds.set(key, { loading: true, list: [] });
    let list = [];
    try {
      list = await api.versions.loaderVersions(row.loader, row.gameVersion);
    } catch {
      // The backend falls back to the latest stable build.
    }
    loaderBuilds.set(key, { loading: false, list });
    if (modal.closed) return;
    for (const [rowId, other] of rows) {
      if (`${other.loader}|${other.gameVersion}` !== key) continue;
      preselect(other, list);
      if (other.expanded) renderRow(rowId);
    }
  }

  /** Newest stable build when the source pinned none (or one this loader lacks). */
  function preselect(row, list) {
    if (list.length && (!row.loaderVersion || !list.some((b) => b.version === row.loaderVersion))) {
      row.loaderVersion = (list.find((b) => b.stable) ?? list[0]).version;
    }
  }

  async function browseFolder() {
    const folder = await pickDirectory("Choose a launcher's data folder or an instance folder");
    if (!folder || modal.closed) return;
    browsing = true;
    renderBar();
    try {
      const found = await api.importApi.inspect(folder);
      if (modal.closed) return;
      if (!found.length) {
        toast('Nothing importable was found in that folder.', { kind: 'error' });
      } else {
        absorb(found);
        toast(`Found ${plural(found.length, 'importable instance')}.`, { kind: 'success' });
      }
    } catch (error) {
      toast(errorText(error), { kind: 'error' });
    } finally {
      browsing = false;
    }
    renderList();
  }

  async function runImport() {
    const list = requests();
    if (!list.length || importing || list.some((r) => !r.gameVersion)) return;
    importing = true;
    modal.setDismissable(false);
    offProgress = store.on('progress', renderProgress);
    renderBar();
    let closeAfter = false;
    try {
      const outcome = await api.importApi.importInstances(list);
      if (outcome.imported.length) {
        await refreshInstances();
        toast(`Imported ${plural(outcome.imported.length, 'instance')}.`, { kind: 'success' });
      }
      // One broken instance must not hide the ones that succeeded.
      for (const failure of outcome.failed) toast(`${failure.name}: ${failure.error}`, { kind: 'error' });
      closeAfter = outcome.imported.length > 0 && outcome.failed.length === 0;
    } catch (error) {
      toast(errorText(error), { kind: 'error' });
    } finally {
      importing = false;
      offProgress?.();
      offProgress = null;
      modal.setDismissable(true);
    }
    if (closeAfter) return modal.close();
    if (!modal.closed) {
      renderProgress();
      renderBar();
    }
  }

  // ----- events -----

  const rowOf = (target) => {
    const node = target.closest('[data-row]');
    return node ? [node.dataset.row, rows.get(node.dataset.row)] : [null, null];
  };

  body.addEventListener('click', async (event) => {
    const target = event.target;
    if (target.closest('[data-browse]')) return browseFolder();
    if (target.closest('[data-snapshots]')) {
      showSnapshots = true;
      return renderList();
    }
    const [id, row] = rowOf(target);
    if (!row) return;
    if (target.closest('[data-expand]')) {
      row.expanded = !row.expanded;
      renderRow(id);
      if (row.expanded) loadBuilds(id);
      return;
    }
    const iconBtn = target.closest('[data-pick-icon]');
    if (iconBtn) {
      const chosen = await pickIcon(iconBtn, row.icon);
      if (chosen !== undefined && !modal.closed) {
        row.icon = chosen;
        renderRow(id);
      }
    }
  });

  body.addEventListener('input', (event) => {
    if (!event.target.matches('[data-name]')) return;
    const [id, row] = rowOf(event.target);
    if (!row) return;
    row.name = event.target.value;
    // Update in place: re-rendering the row would steal the focus.
    const title = listEl.querySelector(`[data-row="${CSS.escape(id)}"] [data-title]`);
    if (title) title.textContent = row.name || candidates.find((c) => c.id === id)?.name || '';
  });

  body.addEventListener('change', (event) => {
    const target = event.target;
    const [id, row] = rowOf(target);
    if (!row) return;
    if (target.matches('[data-check]')) {
      row.checked = target.checked;
      target.closest('[data-row]').classList.toggle('is-unchecked', !row.checked);
      return renderBar();
    }
    if (target.matches('[data-game-version]')) {
      row.gameVersion = target.value;
      renderRow(id);
      return loadBuilds(id);
    }
    if (target.matches('[data-loader]')) {
      row.loader = target.value;
      // A different loader invalidates the build.
      row.loaderVersion = null;
      renderRow(id);
      return loadBuilds(id);
    }
    if (target.matches('[data-loader-version]')) row.loaderVersion = target.value || null;
  });

  importBtn.addEventListener('click', runImport);

  // ----- load -----

  renderList();

  api.importApi
    .scan()
    .then((scan) => {
      if (modal.closed) return;
      absorb(scan.candidates);
      sourceNotes = scan.sources.filter((s) => s.found).map((s) => `${s.label}: ${s.path}`);
    })
    .catch((error) => toast(errorText(error), { kind: 'error' }))
    .finally(() => {
      scanning = false;
      renderList();
    });

  // Only needed to *change* a version; candidates keep their own otherwise.
  api.versions
    .list(true)
    .then((list) => {
      if (modal.closed) return;
      gameVersions = list;
      for (const [id, row] of rows) if (row.expanded) renderRow(id);
    })
    .catch(() => {});
}
