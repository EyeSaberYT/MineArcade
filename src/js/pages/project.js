// Project page: a Modrinth or CurseForge project with its gallery,
// description, versions and install actions.
import * as api from '../api.js';
import { gameVersionSummary, openInstallDialog, projectIcon, versionTypeTone } from '../dialogs/install.js';
import { mountMarkdown } from '../markdown.js';
import { setCrumb } from '../router.js';
import { store } from '../store.js';
import { LAST_DISCOVER_KEY } from './discover.js';
import {
  compactNumber,
  emptyState,
  errorBlock,
  formatBytes,
  formatDate,
  html,
  icon,
  instanceIcon,
  loaderLabel,
  projectTypeBadge,
  relativeTime,
  setHtml,
  toastError,
} from '../ui.js';

const VERSIONS_STEP = 25;
const CURSEFORGE_PATHS = {
  mod: 'mc-mods',
  modpack: 'modpacks',
  resourcepack: 'texture-packs',
  shader: 'shaders',
  datapack: 'data-packs',
};

function webUrl(project, source) {
  if (source === 'curseforge') {
    return `https://www.curseforge.com/minecraft/${CURSEFORGE_PATHS[project.projectType] ?? 'mc-mods'}/${encodeURIComponent(project.slug)}`;
  }
  return `https://modrinth.com/${encodeURIComponent(project.projectType)}/${encodeURIComponent(project.slug || project.id)}`;
}

const sideLabel = (value) => ({ required: 'Required', optional: 'Optional', unsupported: 'Unsupported' })[value] ?? value;
const versionLabel = (v) => v.versionNumber || v.name || v.id;
const unique = (items) => [...new Set(items)];
// loaderLabel() maps anything unknown (iris, optifine, datapack…) to Vanilla; show those as-is.
const loaderName = (l) => (loaderLabel(l) === 'Vanilla' && l !== 'vanilla' ? l : loaderLabel(l));

export function render(root, params, query) {
  let disposed = false;
  const source = params.source === 'curseforge' ? 'curseforge' : 'modrinth';
  const id = source === 'curseforge' && !params.id.startsWith('cf:') ? `cf:${params.id}` : params.id;
  let instanceId = query.get('instance') ?? '';

  let project = null;
  let versions = [];
  let tab = 'description';
  let galleryIndex = 0;
  let filterGame = 'any';
  let filterLoader = 'any';
  let shownVersions = VERSIONS_STEP;
  const expanded = new Set();
  let descriptionMounted = false;

  const author = (() => {
    try {
      return sessionStorage.getItem(`minearcade.author.${id}`) ?? '';
    } catch {
      return '';
    }
  })();

  const targetInstance = () => (instanceId ? store.state.instances.find((i) => i.id === instanceId) ?? null : null);

  function backPath() {
    let last = null;
    try {
      last = sessionStorage.getItem(LAST_DISCOVER_KEY);
    } catch {
      // Fall through.
    }
    if (last) return last;
    const type = project?.projectType ?? 'mod';
    const q = new URLSearchParams({ source });
    if (instanceId) q.set('instance', instanceId);
    return `/discover/${type}?${q}`;
  }

  function drawLoading() {
    setHtml(
      root,
      html`
        <div class="skeleton" style="height:180px"></div>
        <div class="skeleton" style="height:360px"></div>`,
    );
  }

  function install(versionId) {
    if (!project) return;
    openInstallDialog(project, { instanceId: instanceId || undefined, versionId });
  }

  function linkButtons() {
    const links = [
      ['Source', project.sourceUrl, 'terminal'],
      ['Issues', project.issuesUrl, 'alert'],
      ['Wiki', project.wikiUrl, 'globe'],
      ['Discord', project.discordUrl, 'link'],
    ].filter(([, url]) => url && /^https?:/i.test(url));
    return links.map(
      ([label, url, iconName]) => html`<button class="btn btn-ghost btn-sm" data-url="${url}" title="${url}">${icon(iconName, 'icon-sm')} ${label}</button>`,
    );
  }

  function drawPage() {
    setCrumb(project.title);
    const instance = targetInstance();
    const gallery = project.gallery ?? [];
    const isModpack = project.projectType === 'modpack';
    setHtml(
      root,
      html`

        ${instanceId &&
        instance &&
        html`<div class="dc-banner">
          ${instanceIcon(instance, 36)}
          <div class="grow"><div>Installing into <strong>${instance.name}</strong></div><div class="small muted">${instance.gameVersion} · ${loaderLabel(instance.loader)}</div></div>
          <button class="btn btn-outline btn-sm" data-clear-instance>${icon('x', 'icon-sm')} Clear</button>
        </div>`}

        <section class="card dc-hero">
          ${projectIcon(project, 96)}
          <div class="grow">
            <div class="row-wrap" style="gap:6px">
              ${projectTypeBadge(project.projectType)}
              <span class="badge">${source === 'curseforge' ? 'CurseForge' : 'Modrinth'}</span>
            </div>
            <h1 class="dc-hero-title selectable">${project.title}</h1>
            ${author && html`<div class="small muted">by ${author}</div>`}
            <p class="dc-hero-desc">${project.description}</p>
            <div class="dc-meta">
              <span>${icon('download', 'icon-sm')} ${compactNumber(project.downloads)} downloads</span>
              ${project.followers > 0 && html`<span>${icon('heart', 'icon-sm')} ${compactNumber(project.followers)} followers</span>`}
              ${project.updated && html`<span>${icon('clock', 'icon-sm')} Updated ${relativeTime(project.updated)}</span>`}
              ${project.license && (project.license.name || project.license.id) && html`<span>${icon('file', 'icon-sm')} ${project.license.name || project.license.id}</span>`}
            </div>
          </div>
          <div class="dc-hero-actions">
            <button class="btn btn-primary btn-lg" data-install-latest ${versions.length ? '' : 'disabled'}>${icon(isModpack ? 'package' : 'download')} ${isModpack ? 'Install modpack' : 'Install'}</button>
            <button class="btn btn-outline btn-sm" data-url="${webUrl(project, source)}">${icon('external', 'icon-sm')} View on ${source === 'curseforge' ? 'CurseForge' : 'Modrinth'}</button>
            <div class="row-wrap" style="gap:2px;justify-content:center">${linkButtons()}</div>
          </div>
        </section>

        ${gallery.length > 0 &&
        html`<section class="dc-gallery">
          <figure class="dc-gallery-main">
            <img src="${gallery[galleryIndex]?.url}" alt="${gallery[galleryIndex]?.title ?? project.title}" referrerpolicy="no-referrer">
            ${(gallery[galleryIndex]?.title || gallery[galleryIndex]?.description) &&
            html`<figcaption><strong>${gallery[galleryIndex].title}</strong> ${gallery[galleryIndex].description}</figcaption>`}
          </figure>
          ${gallery.length > 1 &&
          html`<div class="dc-thumbs">${gallery.map(
            (image, index) => html`<button class="dc-thumb ${index === galleryIndex ? 'is-active' : ''}" data-gallery="${index}" aria-label="Show screenshot ${index + 1}"><img src="${image.url}" alt="" loading="lazy" referrerpolicy="no-referrer"></button>`,
          )}</div>`}
        </section>`}

        <div class="dc-detail">
          <section class="card dc-main">
            <nav class="tabs">
              <button class="tab ${tab === 'description' ? 'active' : ''}" data-tab="description">Description</button>
              <button class="tab ${tab === 'versions' ? 'active' : ''}" data-tab="versions">Versions <span class="count">${versions.length}</span></button>
            </nav>
            <div class="dc-tab-body" data-description ${tab === 'description' ? '' : 'hidden'}></div>
            <div class="dc-tab-body" data-versions ${tab === 'versions' ? '' : 'hidden'}></div>
          </section>
          <aside class="stack">
            <section class="panel stack-sm">
              <h3>Project details</h3>
              <dl class="dc-dl">
                ${project.clientSide && html`<dt>Client</dt><dd>${sideLabel(project.clientSide)}</dd>`}
                ${project.serverSide && html`<dt>Server</dt><dd>${sideLabel(project.serverSide)}</dd>`}
                ${project.published && html`<dt>Published</dt><dd title="${formatDate(project.published)}">${relativeTime(project.published)}</dd>`}
                ${project.updated && html`<dt>Updated</dt><dd title="${formatDate(project.updated)}">${relativeTime(project.updated)}</dd>`}
                <dt>License</dt><dd>${project.license?.name || project.license?.id || 'Not listed'}</dd>
                <dt>Versions</dt><dd>${versions.length}</dd>
              </dl>
            </section>
            ${Boolean(project.loaders?.length || project.gameVersions?.length) &&
            html`<section class="panel stack-sm">
              <h3>Compatibility</h3>
              ${project.loaders?.length > 0 && html`<div class="row-wrap" style="gap:6px">${project.loaders.map((l) => html`<span class="badge">${loaderName(l)}</span>`)}</div>`}
              ${project.gameVersions?.length > 0 && html`<p class="small muted">Minecraft ${gameVersionSummary(project.gameVersions.filter((v) => /^\d/.test(v)), 6)}</p>`}
            </section>`}
            ${project.categories?.length > 0 &&
            html`<section class="panel stack-sm">
              <h3>Categories</h3>
              <div class="dc-chips">${unique([...project.categories, ...(project.additionalCategories ?? [])]).map((c) => html`<span class="dc-tag">${c}</span>`)}</div>
            </section>`}
          </aside>
        </div>`,
    );
    descriptionMounted = false;
    drawTab();
  }

  function drawTab() {
    if (tab === 'description') {
      if (descriptionMounted) return;
      descriptionMounted = true;
      const slot = root.querySelector('[data-description]');
      if (!project.body?.trim()) {
        setHtml(slot, html`<p class="muted">No description provided.</p>`);
        return;
      }
      setHtml(slot, html`<div class="skeleton" style="height:240px"></div>`);
      mountMarkdown(slot, project.body, { isStale: () => disposed }).catch((error) => {
        if (!disposed) setHtml(slot, html`<p class="form-error">Couldn't render the description: ${String(error?.message ?? error)}</p>`);
      });
      return;
    }
    drawVersions();
  }

  function filteredVersions() {
    return versions.filter(
      (v) =>
        (filterGame === 'any' || (v.gameVersions ?? []).includes(filterGame)) &&
        (filterLoader === 'any' || (v.loaders ?? []).includes(filterLoader)),
    );
  }

  function drawVersions() {
    const slot = root.querySelector('[data-versions]');
    if (!slot) return;
    if (!versions.length) {
      setHtml(slot, emptyState({ iconName: 'package', title: 'No versions listed', description: 'This project has no downloadable files yet.' }));
      return;
    }
    const games = unique(versions.flatMap((v) => v.gameVersions ?? [])).sort((a, b) => b.localeCompare(a, 'en', { numeric: true }));
    const loaders = unique(versions.flatMap((v) => v.loaders ?? []));
    const list = filteredVersions();
    setHtml(
      slot,
      html`
        <div class="row-wrap dc-version-filters">
          <select class="input" data-filter-game aria-label="Game version">
            <option value="any">All game versions</option>
            ${games.map((g) => html`<option value="${g}" ${g === filterGame ? 'selected' : ''}>${g}</option>`)}
          </select>
          ${loaders.length > 1 &&
          html`<select class="input" data-filter-loader aria-label="Loader">
            <option value="any">All loaders</option>
            ${loaders.map((l) => html`<option value="${l}" ${l === filterLoader ? 'selected' : ''}>${loaderName(l)}</option>`)}
          </select>`}
          <span class="small muted">${list.length} of ${versions.length}</span>
        </div>
        ${list.length === 0
          ? html`<p class="muted small">No versions match these filters.</p>`
          : html`<div class="list">
              ${list.slice(0, shownVersions).map((v) => {
                const primary = (v.files ?? []).find((f) => f.primary) ?? v.files?.[0];
                const open = expanded.has(v.id);
                return html`<div class="dc-version ${open ? 'is-open' : ''}">
                  <div class="list-row">
                    <span class="dc-vtype dc-compat-${versionTypeTone[v.versionType] ?? 'ok'}" title="${v.versionType}">${(v.versionType || 'release').slice(0, 1).toUpperCase()}</span>
                    <div class="grow">
                      <div class="truncate bold">${versionLabel(v)}${v.name && v.name !== v.versionNumber ? html` <span class="muted" style="font-weight:400">${v.name}</span>` : ''}</div>
                      <div class="tiny muted truncate">${(v.loaders ?? []).map((l) => loaderName(l)).join(', ') || 'All loaders'} · ${gameVersionSummary(v.gameVersions)}${v.datePublished ? ` · ${relativeTime(v.datePublished)}` : ''}${v.downloads ? ` · ${compactNumber(v.downloads)} downloads` : ''}${primary?.size ? ` · ${formatBytes(primary.size)}` : ''}</div>
                    </div>
                    <div class="row-actions">
                      <button class="btn btn-ghost btn-sm" data-changelog="${v.id}" aria-expanded="${open}">${icon(open ? 'chevron-up' : 'chevron-down', 'icon-sm')} Changelog</button>
                      <button class="btn btn-outline btn-sm" data-install-version="${v.id}">${icon('download', 'icon-sm')} Install</button>
                    </div>
                  </div>
                  ${open && html`<div class="dc-changelog" data-changelog-body="${v.id}"></div>`}
                </div>`;
              })}
            </div>
            ${list.length > shownVersions && html`<button class="btn btn-ghost btn-sm" data-more-versions style="align-self:center">Show ${Math.min(VERSIONS_STEP, list.length - shownVersions)} more</button>`}`}`,
    );
    for (const body of slot.querySelectorAll('[data-changelog-body]')) {
      const version = versions.find((v) => v.id === body.dataset.changelogBody);
      if (!version?.changelog?.trim()) {
        setHtml(body, html`<p class="muted small">No changelog provided.</p>`);
        continue;
      }
      mountMarkdown(body, version.changelog, { isStale: () => disposed }).catch(() => {
        if (!disposed) body.textContent = version.changelog;
      });
    }
  }

  root.addEventListener('click', (event) => {
    const target = event.target;
    const url = target.closest('[data-url]');
    if (url) return void api.system.openUrl(url.dataset.url).catch(toastError);
    if (target.closest('[data-install-latest]')) return install(undefined);
    const versionBtn = target.closest('[data-install-version]');
    if (versionBtn) return install(versionBtn.dataset.installVersion);
    const tabBtn = target.closest('[data-tab]');
    if (tabBtn) {
      tab = tabBtn.dataset.tab;
      root.querySelectorAll('[data-tab]').forEach((b) => b.classList.toggle('active', b.dataset.tab === tab));
      root.querySelector('[data-description]').hidden = tab !== 'description';
      root.querySelector('[data-versions]').hidden = tab !== 'versions';
      return drawTab();
    }
    const thumb = target.closest('[data-gallery]');
    if (thumb) {
      galleryIndex = Number(thumb.dataset.gallery);
      const image = project.gallery[galleryIndex];
      const main = root.querySelector('.dc-gallery-main');
      setHtml(
        main,
        html`<img src="${image.url}" alt="${image.title ?? project.title}" referrerpolicy="no-referrer">${(image.title || image.description) && html`<figcaption><strong>${image.title}</strong> ${image.description}</figcaption>`}`,
      );
      root.querySelectorAll('[data-gallery]').forEach((b) => b.classList.toggle('is-active', Number(b.dataset.gallery) === galleryIndex));
      return;
    }
    const changelog = target.closest('[data-changelog]');
    if (changelog) {
      const versionId = changelog.dataset.changelog;
      if (expanded.has(versionId)) expanded.delete(versionId);
      else expanded.add(versionId);
      return drawVersions();
    }
    if (target.closest('[data-more-versions]')) {
      shownVersions += VERSIONS_STEP;
      return drawVersions();
    }
    if (target.closest('[data-clear-instance]')) {
      instanceId = '';
      history.replaceState(null, '', `#/project/${source}/${encodeURIComponent(id)}`);
      return drawPage();
    }
    if (target.closest('#dc-retry')) return load();
  });

  root.addEventListener('change', (event) => {
    if (event.target.matches('[data-filter-game]')) {
      filterGame = event.target.value;
      shownVersions = VERSIONS_STEP;
      drawVersions();
    } else if (event.target.matches('[data-filter-loader]')) {
      filterLoader = event.target.value;
      shownVersions = VERSIONS_STEP;
      drawVersions();
    }
  });

  async function load() {
    drawLoading();
    try {
      const [nextProject, nextVersions] = await Promise.all([
        api.modrinth.project(id),
        api.modrinth.versions(id).catch(() => []),
      ]);
      if (disposed) return;
      project = nextProject;
      versions = nextVersions ?? [];
      galleryIndex = Math.max(0, (project.gallery ?? []).findIndex((g) => g.featured));
      // Pre-filter the versions list to the target instance.
      const instance = targetInstance();
      if (instance) {
        if (versions.some((v) => (v.gameVersions ?? []).includes(instance.gameVersion))) filterGame = instance.gameVersion;
        if (versions.some((v) => (v.loaders ?? []).includes(instance.loader))) filterLoader = instance.loader;
      }
      drawPage();
    } catch (error) {
      if (disposed) return;
      setHtml(
        root,
        html`<a class="back-link" href="#${backPath()}">${icon('arrow-left', 'icon-sm')} Back to Discover</a>
          ${errorBlock(error, 'dc-retry')}`,
      );
    }
  }

  const off = store.on('instances', () => {
    if (project && instanceId && !root.querySelector('.dc-banner')) drawPage();
  });

  load();

  return () => {
    disposed = true;
    off();
  };
}
