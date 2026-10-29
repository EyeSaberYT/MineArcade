// Install dialogs for Modrinth / CurseForge projects.
//   openInstallDialog(project, options)  — content into an existing instance
//                                          (modpacks are routed to the pack flow)
//   openModpackInstallDialog(project, options) — a modpack as a brand new instance
// `project` only needs { id, title, projectType, iconUrl }; a search hit's
// `projectId` is accepted in place of `id`. CurseForge ids carry a `cf:` prefix.
import * as api from '../api.js';
import { refreshInstances } from '../actions.js';
import { navigate } from '../router.js';
import { store } from '../store.js';
import { errorText } from '../tauri.js';
import {
  emptyState,
  formatBytes,
  html,
  icon,
  instanceIcon,
  loaderBadge,
  loaderLabel,
  openModal,
  projectTypeLabel,
  relativeTime,
  setHtml,
  spinner,
  toast,
} from '../ui.js';

// ---------- Shared helpers (also used by the Discover and project pages) ----------

/** 'curseforge' for `cf:` ids, else 'modrinth'. */
export const sourceOf = (id) => (String(id ?? '').startsWith('cf:') ? 'curseforge' : 'modrinth');

/** Hash path of a project's page, keeping the target instance. */
export function projectPath(id, instanceId) {
  const suffix = instanceId ? `?instance=${encodeURIComponent(instanceId)}` : '';
  return `/project/${sourceOf(id)}/${encodeURIComponent(id)}${suffix}`;
}

/** A project's icon, falling back to its initial. */
export function projectIcon(project, size = 48) {
  const style = `width:${size}px;height:${size}px;font-size:${Math.round(size / 2.6)}px`;
  const initial = String(project?.title ?? '?').trim().slice(0, 1).toUpperCase() || '?';
  const fallback = html`<div class="dc-icon" style="${style}">${initial}</div>`;
  if (!project?.iconUrl) return fallback;
  return html`<div class="dc-icon" style="${style}"><img src="${project.iconUrl}" alt="" loading="lazy" referrerpolicy="no-referrer" data-fallback="${String(fallback)}"></div>`;
}

const LOADER_AGNOSTIC = new Set(['resourcepack', 'shader']);
const LOADER_TYPES = new Set(['mod', 'modpack', 'datapack']);

/** Whether a loader filter applies to a project type. */
export const loaderApplies = (projectType) => LOADER_TYPES.has(projectType);

function loaderMatches(versionLoaders, instanceLoader, projectType) {
  if (LOADER_AGNOSTIC.has(projectType)) return true;
  const loaders = (versionLoaders ?? []).map((l) => String(l).toLowerCase());
  if (!loaders.length) return true;
  if (projectType === 'datapack' && loaders.includes('datapack')) return true;
  if (loaders.includes(instanceLoader)) return true;
  // Quilt loads Fabric mods.
  return instanceLoader === 'quilt' && loaders.includes('fabric');
}

function gameMatches(versionGames, gameVersion) {
  return !versionGames?.length || versionGames.includes(gameVersion);
}

/** Versions of a project that suit an instance (game version and loader). */
export function compatibleVersions(versions, instance, projectType) {
  return versions.filter(
    (v) => gameMatches(v.gameVersions, instance.gameVersion) && loaderMatches(v.loaders, instance.loader, projectType),
  );
}

/**
 * How well an instance suits a project:
 *   { level: 'ok' | 'warn' | 'no', reason, versions }
 * where `versions` are the candidate versions for that instance.
 */
export function instanceCompatibility(instance, versions, projectType) {
  if (projectType === 'mod' && instance.loader === 'vanilla') {
    return { level: 'no', reason: "Vanilla instances can't load mods", versions: [] };
  }
  if (LOADER_AGNOSTIC.has(projectType)) {
    // Packs aren't tied to a loader and often work across adjacent releases,
    // so every version is offered; matching ones only decide the default.
    const matching = versions.filter((v) => gameMatches(v.gameVersions, instance.gameVersion));
    if (projectType === 'shader' && instance.loader === 'vanilla') {
      return { level: 'warn', reason: 'Shaders need Iris or OptiFine', versions };
    }
    if (!matching.length) {
      return { level: 'warn', reason: `No version lists Minecraft ${instance.gameVersion}`, versions };
    }
    return { level: 'ok', reason: `Supports ${instance.gameVersion}`, versions };
  }
  const list = compatibleVersions(versions, instance, projectType);
  if (list.length) return { level: 'ok', reason: `${list.length} compatible ${list.length === 1 ? 'version' : 'versions'}`, versions: list };
  const anyGame = versions.some((v) => gameMatches(v.gameVersions, instance.gameVersion));
  return {
    level: 'no',
    reason: anyGame
      ? `Not available for ${loaderLabel(instance.loader)}`
      : `Not available for Minecraft ${instance.gameVersion}`,
    versions: [],
  };
}

const versionLabel = (v) => v.versionNumber || v.name || v.id;

const typeTone = { release: 'ok', beta: 'warn', alpha: 'no' };

/** Summary for a version's supported game versions: `1.20.1, 1.20.2 +3`. */
export function gameVersionSummary(list, shown = 3) {
  if (!list?.length) return 'Any version';
  const recent = [...list].reverse();
  return recent.length > shown ? `${recent.slice(0, shown).join(', ')} +${recent.length - shown}` : recent.join(', ');
}

// ---------- Content into an instance ----------

/**
 * options: { instanceId, versionId, onInstalled(records, instanceId) }
 * Modpacks open the modpack flow instead.
 */
export function openInstallDialog(input, options = {}) {
  const project = normalise(input);
  if (project.projectType === 'modpack') return openModpackInstallDialog(project, options);

  let versions = null;
  let versionsError = null;
  let instanceId = options.instanceId ?? '';
  let versionId = options.versionId ?? '';
  let installedMap = new Map();
  let depRequest = 0;
  let busy = false;
  let off = () => {};

  const typeLabel = projectTypeLabel(project.projectType).toLowerCase();
  const modal = openModal({
    title: `Install ${typeLabel}`,
    size: 'md',
    onClose: () => off(),
    body: html`
      <div class="row dc-dialog-head">${projectIcon(project, 44)}<div class="grow"><strong class="truncate" style="display:block">${project.title}</strong><span class="small muted">${projectTypeLabel(project.projectType)} · ${project.source === 'curseforge' ? 'CurseForge' : 'Modrinth'}</span></div></div>
      <div data-content>${html`<div class="loading-block">${spinner()} Checking compatibility…</div>`}</div>`,
    footer: html`
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" data-install disabled>${icon('download')} Install</button>`,
  });

  const content = modal.body.querySelector('[data-content]');
  const installBtn = modal.foot.querySelector('[data-install]');

  const instances = () => store.state.instances;
  const compat = (instance) => instanceCompatibility(instance, versions ?? [], project.projectType);

  function pickDefaultInstance() {
    const list = instances();
    if (instanceId && list.some((i) => i.id === instanceId)) return;
    const ranked = [...list].sort((a, b) => new Date(b.lastPlayed ?? 0) - new Date(a.lastPlayed ?? 0));
    instanceId = (ranked.find((i) => compat(i).level === 'ok') ?? ranked.find((i) => compat(i).level === 'warn'))?.id ?? '';
  }

  function candidates(instance) {
    const result = compat(instance);
    if (!LOADER_AGNOSTIC.has(project.projectType)) return result.versions;
    // Matching game versions first, then the rest, newest first within each.
    const match = (v) => gameMatches(v.gameVersions, instance.gameVersion);
    return [...result.versions.filter(match), ...result.versions.filter((v) => !match(v))];
  }

  function pickDefaultVersion(instance) {
    const list = candidates(instance);
    if (versionId && list.some((v) => v.id === versionId)) return;
    const stable = list.find((v) => v.versionType === 'release');
    versionId = (stable ?? list[0])?.id ?? '';
  }

  function draw() {
    if (modal.closed) return;
    const list = instances();

    if (versionsError) {
      setHtml(content, html`<p class="form-error">Couldn't load versions: ${versionsError}</p>`);
      installBtn.disabled = true;
      return;
    }
    if (!versions) return;
    if (!versions.length) {
      setHtml(content, emptyState({ iconName: 'package', title: 'No files published', description: `${project.title} has no downloadable versions yet.` }));
      installBtn.disabled = true;
      return;
    }
    if (!list.length) {
      setHtml(
        content,
        emptyState({
          iconName: 'layers',
          title: "You don't have any instances yet",
          description: `Create one to install ${project.title} into.`,
          actions: html`<button class="btn btn-primary btn-sm" data-new-instance>${icon('plus')} New instance</button>`,
        }),
      );
      installBtn.disabled = true;
      return;
    }

    const selected = list.find((i) => i.id === instanceId) ?? null;
    const anyUsable = list.some((i) => compat(i).level !== 'no');
    if (selected) pickDefaultVersion(selected);
    const selectedCompat = selected ? compat(selected) : null;
    const versionList = selected ? candidates(selected) : [];

    setHtml(
      content,
      html`
        <div class="stack">
          <div class="field">
            <span>Install into</span>
            ${!anyUsable &&
            html`<div class="dc-notice dc-notice-warn">${icon('alert', 'icon-sm')}<span>None of your instances can run ${project.title}. ${newInstanceHint(versions, project.projectType)}</span></div>`}
            <div class="dc-instance-list" role="listbox">
              ${list.map((instance) => {
                const c = compat(instance);
                return html`<button class="dc-instance ${instance.id === instanceId ? 'is-selected' : ''} ${c.level === 'no' ? 'is-off' : ''}" data-instance="${instance.id}" role="option" aria-selected="${instance.id === instanceId}" ${c.level === 'no' ? 'disabled' : ''}>
                  ${instanceIcon(instance, 32)}
                  <span class="grow">
                    <span class="truncate bold" style="display:block">${instance.name}</span>
                    <span class="tiny muted">${instance.gameVersion} · ${loaderLabel(instance.loader)}</span>
                  </span>
                  <span class="dc-compat dc-compat-${c.level}" title="${c.reason}">${icon(c.level === 'ok' ? 'check-circle' : c.level === 'warn' ? 'alert' : 'x-circle', 'icon-sm')}<span>${c.reason}</span></span>
                </button>`;
              })}
            </div>
            <button class="link-btn small" data-new-instance style="align-self:flex-start">${icon('plus', 'icon-sm')} Create a new instance instead</button>
          </div>

          ${selected &&
          versionList.length > 0 &&
          html`<label class="field">
            <span>Version</span>
            <select class="input" data-version>
              ${versionList.map(
                (v) =>
                  html`<option value="${v.id}" ${v.id === versionId ? 'selected' : ''}>${versionLabel(v)}${v.versionType && v.versionType !== 'release' ? ` (${v.versionType})` : ''}${v.datePublished ? ` · ${relativeTime(v.datePublished)}` : ''}${LOADER_AGNOSTIC.has(project.projectType) && !gameMatches(v.gameVersions, selected.gameVersion) ? ` · for ${gameVersionSummary(v.gameVersions, 2)}` : ''}</option>`,
              )}
            </select>
          </label>`}

          ${selected && selectedCompat.level === 'no' && html`<div class="dc-notice dc-notice-error">${icon('x-circle', 'icon-sm')}<span>${selected.name}: ${selectedCompat.reason}.${selected.loader === 'vanilla' && project.projectType === 'mod' ? ' Pick an instance with Fabric, Quilt, Forge or NeoForge, or create one.' : ' Pick another instance.'}</span></div>`}
          ${selected && selectedCompat.level === 'warn' && html`<div class="dc-notice dc-notice-warn">${icon('alert', 'icon-sm')}<span>${packWarning(project, selected, selectedCompat)}</span></div>`}
          ${selected && store.state.running.has(selected.id) && html`<div class="dc-notice">${icon('info', 'icon-sm')}<span>${selected.name} is running. Restart the game to load the new ${typeLabel}.</span></div>`}
          ${selected && installedMap.get(selected.id)?.has(project.id) && html`<div class="dc-notice">${icon('info', 'icon-sm')}<span>Already installed in ${selected.name}. Installing replaces it with the chosen version.</span></div>`}

          ${selected && versionId && html`<div class="field"><span>Also installs</span><div data-deps class="dc-deps">${html`<div class="small muted row">${spinner('icon-sm')} Resolving dependencies…</div>`}</div></div>`}
          <p class="form-error" data-error></p>
        </div>`,
    );
    installBtn.disabled = busy || !selected || !versionId || selectedCompat?.level === 'no';
    if (selected && versionId) loadDependencies(selected);
  }

  async function loadInstalled(id) {
    if (installedMap.has(id)) return;
    installedMap.set(id, new Set());
    try {
      const full = await api.instances.get(id);
      installedMap.set(id, new Set((full.installedProjects ?? []).map((p) => p.projectId)));
      if (id === instanceId) draw();
    } catch {
      // Only used for hints.
    }
  }

  async function loadDependencies(instance) {
    const request = ++depRequest;
    const slot = () => content.querySelector('[data-deps]');
    try {
      const files = await api.modrinth.dependencies(instance.id, versionId);
      if (request !== depRequest || modal.closed || !slot()) return;
      const installed = installedMap.get(instance.id) ?? new Set();
      const deps = files.filter((f) => f.projectId !== project.id && (f.dependency ?? true));
      setHtml(
        slot(),
        deps.length
          ? html`<div class="list">${deps.map(
              (dep) => html`<div class="list-row dc-dep">
                ${projectIcon({ title: dep.title, iconUrl: dep.iconUrl }, 28)}
                <span class="grow"><span class="truncate" style="display:block">${dep.title || dep.filename}</span><span class="tiny muted truncate" style="display:block">${dep.filename}${dep.size ? ` · ${formatBytes(dep.size)}` : ''}</span></span>
                ${installed.has(dep.projectId) ? html`<span class="badge">Installed</span>` : html`<span class="badge badge-info">Required</span>`}
              </div>`,
            )}</div>`
          : html`<p class="small muted">No other files are needed.</p>`,
      );
    } catch (error) {
      if (request !== depRequest || !slot()) return;
      setHtml(slot(), html`<p class="small muted">Couldn't preview dependencies (${errorText(error)}). Required ones are still installed.</p>`);
    }
  }

  modal.body.addEventListener('click', async (event) => {
    if (busy) return;
    const choice = event.target.closest('[data-instance]');
    if (choice && !choice.disabled) {
      instanceId = choice.dataset.instance;
      versionId = '';
      loadInstalled(instanceId);
      return draw();
    }
    if (event.target.closest('[data-new-instance]')) {
      const { openNewInstanceDialog } = await import('./new-instance.js');
      openNewInstanceDialog({
        onCreated: async (created) => {
          await refreshInstances();
          if (modal.closed) return;
          instanceId = created.id;
          versionId = '';
          draw();
        },
      });
    }
  });

  modal.body.addEventListener('change', (event) => {
    if (event.target.matches('[data-version]')) {
      versionId = event.target.value;
      const selected = instances().find((i) => i.id === instanceId);
      if (selected) loadDependencies(selected);
    }
  });

  installBtn.addEventListener('click', async () => {
    const instance = instances().find((i) => i.id === instanceId);
    if (!instance || !versionId) return;
    const errorEl = content.querySelector('[data-error]');
    if (errorEl) errorEl.textContent = '';
    busy = true;
    installBtn.disabled = true;
    installBtn.innerHTML = `${spinner()} Installing…`;
    modal.setDismissable(false);
    try {
      const records = await api.modrinth.install(instance.id, project.id, versionId);
      await refreshInstances();
      modal.setDismissable(true);
      modal.close();
      const extra = Math.max(0, (records?.length ?? 1) - 1);
      toast(`Installed ${project.title}`, {
        kind: 'success',
        description: `Into ${instance.name}${extra ? ` with ${extra} ${extra === 1 ? 'dependency' : 'dependencies'}` : ''}.`,
        actions: [{ label: 'Open instance', onClick: () => navigate(`/instance/${instance.id}/content`) }],
        duration: 7000,
      });
      options.onInstalled?.(records, instance.id);
    } catch (error) {
      busy = false;
      modal.setDismissable(true);
      installBtn.innerHTML = `${icon('download')} Install`;
      installBtn.disabled = false;
      const target = content.querySelector('[data-error]');
      if (target) target.textContent = errorText(error);
      else toast(errorText(error), { kind: 'error' });
    }
  });

  off = store.on(['instances', 'running'], () => !busy && draw());

  (async () => {
    try {
      const loaded = await api.modrinth.versions(project.id);
      if (modal.closed) return;
      versions = loaded;
    } catch (error) {
      if (modal.closed) return;
      versionsError = errorText(error);
    }
    if (versions) pickDefaultInstance();
    if (instanceId) loadInstalled(instanceId);
    draw();
  })();

  return modal;
}

function newInstanceHint(versions, projectType) {
  const latest = versions[0];
  if (!latest) return '';
  const game = [...(latest.gameVersions ?? [])].pop();
  const loader = loaderApplies(projectType)
    ? (latest.loaders ?? []).find((l) => ['fabric', 'quilt', 'forge', 'neoforge'].includes(l))
    : null;
  if (!game) return 'Create a new instance for it.';
  return `Create a new instance with Minecraft ${game}${loader ? ` and ${loaderLabel(loader)}` : ''}.`;
}

function packWarning(project, instance, compat) {
  const label = projectTypeLabel(project.projectType);
  if (project.projectType === 'shader' && instance.loader === 'vanilla') {
    return `${label}s need Iris (Fabric/Quilt) or OptiFine to load. ${instance.name} has no mod loader, so it won't use this until one is added.`;
  }
  return `${label}s aren't tied to a mod loader, so every version is listed. ${compat.reason}; check the pack supports Minecraft ${instance.gameVersion} before installing.`;
}

// ---------- Modpacks ----------

/** options: { versionId } — install a modpack as a new instance and open it. */
export function openModpackInstallDialog(input, options = {}) {
  const project = normalise(input);
  let busy = false;

  const modal = openModal({
    title: 'Install modpack',
    description: 'The pack creates its own instance with the right Minecraft version and loader.',
    body: html`
      <div class="row dc-dialog-head">${projectIcon(project, 44)}<div class="grow"><strong class="truncate" style="display:block">${project.title}</strong><span class="small muted">Modpack · ${project.source === 'curseforge' ? 'CurseForge' : 'Modrinth'}</span></div></div>
      <label class="field">
        <span>Instance name</span>
        <input class="input" data-name maxlength="64" value="${project.title}" autofocus>
      </label>
      <label class="field" data-game-field hidden>
        <span>Minecraft version</span>
        <select class="input" data-game></select>
      </label>
      <label class="field">
        <span>Version</span>
        <select class="input" data-version disabled><option>Loading…</option></select>
      </label>
      <div class="row-wrap" data-version-info style="gap:6px"></div>
      <p class="form-error" data-error></p>`,
    footer: html`
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" data-create disabled>${icon('package')} Create instance</button>`,
  });

  const nameInput = modal.body.querySelector('[data-name]');
  const gameField = modal.body.querySelector('[data-game-field]');
  const gameSelect = modal.body.querySelector('[data-game]');
  const select = modal.body.querySelector('[data-version]');
  const info = modal.body.querySelector('[data-version-info]');
  const errorEl = modal.body.querySelector('[data-error]');
  const createBtn = modal.foot.querySelector('[data-create]');
  let versions = [];

  // The pack's own version number says nothing about the game, so every entry
  // also names the Minecraft version and loader it installs.
  const optionLabel = (v) =>
    [
      `${versionLabel(v)}${v.versionType && v.versionType !== 'release' ? ` (${v.versionType})` : ''}`,
      v.gameVersions?.length ? `Minecraft ${gameVersionSummary(v.gameVersions, 2)}` : '',
      v.loaders?.length ? v.loaders.map(loaderLabel).join(', ') : '',
      v.datePublished ? relativeTime(v.datePublished) : '',
    ]
      .filter(Boolean)
      .join(' · ');

  const describe = () => {
    const v = versions.find((entry) => entry.id === select.value);
    setHtml(
      info,
      v
        ? html`${(v.gameVersions ?? []).length
              ? [...v.gameVersions].reverse().map((game) => html`<span class="badge badge-mono">Minecraft ${game}</span>`)
              : html`<span class="badge">Any Minecraft version</span>`}
            ${(v.loaders ?? []).map((loader) => loaderBadge(loader))}
            ${v.datePublished && html`<span class="small muted">published ${relativeTime(v.datePublished)}</span>`}`
        : '',
    );
  };

  /** Fill the version list, limited to one Minecraft version unless `game` is 'any'. */
  const drawVersions = (game, preferredId) => {
    const shown = game === 'any' ? versions : versions.filter((v) => v.gameVersions?.includes(game));
    setHtml(select, shown.map((v) => html`<option value="${v.id}">${optionLabel(v)}</option>`));
    const preferred =
      shown.find((v) => v.id === preferredId) ?? shown.find((v) => v.versionType === 'release') ?? shown[0];
    if (preferred) select.value = preferred.id;
    describe();
  };

  api.modrinth
    .versions(project.id)
    .then((list) => {
      if (modal.closed) return;
      versions = list;
      if (!list.length) {
        setHtml(select, html`<option value="">No versions published</option>`);
        return;
      }
      // Only worth a picker when the pack is published for more than one game version.
      const games = [...new Set(list.flatMap((v) => v.gameVersions ?? []))].sort(compareGameVersions);
      if (games.length > 1) {
        setHtml(
          gameSelect,
          html`<option value="any">All Minecraft versions</option>${games.map((game) => html`<option value="${game}">${game}</option>`)}`,
        );
        gameField.hidden = false;
      }
      drawVersions('any', options.versionId);
      select.disabled = false;
      createBtn.disabled = false;
    })
    .catch((error) => {
      if (modal.closed) return;
      setHtml(select, html`<option value="">Couldn't load versions</option>`);
      errorEl.textContent = errorText(error);
    });

  select.addEventListener('change', describe);
  gameSelect.addEventListener('change', () => drawVersions(gameSelect.value, select.value));

  createBtn.addEventListener('click', async () => {
    if (busy || !select.value) return;
    busy = true;
    errorEl.textContent = '';
    createBtn.disabled = true;
    createBtn.innerHTML = `${spinner()} Installing…`;
    modal.setDismissable(false);
    try {
      const instance = await api.modrinth.installModpack(project.id, select.value, nameInput.value.trim() || project.title);
      await refreshInstances();
      modal.setDismissable(true);
      modal.close();
      toast(`Installed “${instance.name}”`, { kind: 'success' });
      navigate(`/instance/${instance.id}`);
    } catch (error) {
      busy = false;
      modal.setDismissable(true);
      createBtn.disabled = false;
      createBtn.innerHTML = `${icon('package')} Create instance`;
      errorEl.textContent = errorText(error);
    }
  });

  return modal;
}

/** Newest first: 1.21.4 before 1.21 before 1.20.6; snapshots and other odd ids last. */
function compareGameVersions(a, b) {
  const parts = (v) => (/^\d+(\.\d+)*$/.test(v) ? v.split('.').map(Number) : null);
  const pa = parts(a);
  const pb = parts(b);
  if (!pa || !pb) return pa ? -1 : pb ? 1 : b.localeCompare(a);
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const diff = (pb[i] ?? 0) - (pa[i] ?? 0);
    if (diff) return diff;
  }
  return 0;
}

function normalise(input) {
  const id = input.id ?? input.projectId;
  return {
    id,
    title: input.title ?? id,
    projectType: input.projectType ?? 'mod',
    iconUrl: input.iconUrl ?? null,
    source: sourceOf(id),
  };
}

export { typeTone as versionTypeTone };
