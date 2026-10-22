// Discover: search Modrinth or CurseForge, filter, and install content.
// Filters live in the URL query so reloads and back/forward restore them.
import * as api from '../api.js';
import { loaderApplies, openInstallDialog, projectIcon, projectPath } from '../dialogs/install.js';
import { navigate } from '../router.js';
import { prefs, store } from '../store.js';
import {
  compactNumber,
  debounce,
  emptyState,
  errorBlock,
  html,
  icon,
  instanceIcon,
  loaderLabel,
  projectTypeBadge,
  projectTypeLabel,
  relativeTime,
  setHtml,
  toastError,
} from '../ui.js';

const PROJECT_TYPES = ['mod', 'modpack', 'resourcepack', 'shader', 'datapack'];
const SORTS = [
  { value: 'relevance', label: 'Relevance' },
  { value: 'downloads', label: 'Downloads' },
  { value: 'follows', label: 'Followers' },
  { value: 'newest', label: 'Newest' },
  { value: 'updated', label: 'Recently updated' },
];
const CURSEFORGE_SORTS = SORTS.filter((option) => ['relevance', 'downloads', 'updated'].includes(option.value));
const SOURCES = [
  { value: 'modrinth', label: 'Modrinth' },
  { value: 'curseforge', label: 'CurseForge' },
];
const KNOWN_LOADERS = ['fabric', 'quilt', 'forge', 'neoforge'];
const PAGE_SIZE = 20;
const MAX_VERSIONS = 50;
// CurseForge refuses offsets past 10,000 results.
const CURSEFORGE_MAX_RESULTS = 10_000;

/** sessionStorage key holding the last Discover URL, for the project page's back link. */
export const LAST_DISCOVER_KEY = 'minearcade.discover.last';

const sortsFor = (source) => (source === 'curseforge' ? CURSEFORGE_SORTS : SORTS);
// Modrinth files datapack categories under the mod project type.
const categoryType = (type) => (type === 'datapack' ? 'mod' : type);
const categoryLabel = (slug) => String(slug).replace(/[-_]/g, ' ').replace(/^\w/, (c) => c.toUpperCase());

/** Page numbers to show: first, last, and `span` either side of the current page. */
function pageWindow(current, totalPages, span = 2) {
  const pages = new Set([1, totalPages]);
  for (let page = current - span; page <= current + span; page += 1) {
    if (page >= 1 && page <= totalPages) pages.add(page);
  }
  const tokens = [];
  let previous = 0;
  for (const page of [...pages].sort((a, b) => a - b)) {
    if (page - previous > 1) tokens.push('gap');
    tokens.push(page);
    previous = page;
  }
  return tokens;
}

export function render(root, params, query) {
  let disposed = false;
  let request = 0;

  const typeParam = params.type;
  const type = PROJECT_TYPES.includes(typeParam) ? typeParam : prefs.get('discover.type', 'mod');
  const safeType = PROJECT_TYPES.includes(type) ? type : 'mod';
  prefs.set('discover.type', safeType);

  const sourceParam = query.get('source');
  let source = sourceParam === 'curseforge' || sourceParam === 'modrinth' ? sourceParam : prefs.get('discover.source', 'modrinth');
  if (source !== 'curseforge') source = 'modrinth';
  prefs.set('discover.source', source);

  let view = prefs.get('discover.view', 'grid') === 'list' ? 'list' : 'grid';
  let search = query.get('q') ?? '';
  let gameVersion = query.get('version') || 'any';
  let loader = query.get('loader') || 'any';
  let categories = (query.get('categories') ?? '').split(',').filter(Boolean);
  let sort = query.get('sort') ?? 'relevance';
  if (!sortsFor(source).some((s) => s.value === sort)) sort = 'relevance';
  let page = Math.max(1, Number.parseInt(query.get('page') ?? '1', 10) || 1);
  let instanceId = query.get('instance') ?? '';
  let instanceDefaultsApplied = !instanceId || (query.has('version') && query.has('loader'));

  let gameVersions = [];
  let categoryTags = [];
  let hits = [];
  let total = 0;
  let loading = true;
  let error = null;

  const targetInstance = () => (instanceId ? store.state.instances.find((i) => i.id === instanceId) ?? null : null);

  /** Pre-filter to the target instance's version and loader, unless the URL already says otherwise. */
  function applyInstanceDefaults() {
    if (instanceDefaultsApplied) return false;
    const instance = targetInstance();
    if (!instance) return false;
    instanceDefaultsApplied = true;
    if (!query.has('version')) gameVersion = instance.gameVersion;
    if (!query.has('loader') && instance.loader !== 'vanilla' && KNOWN_LOADERS.includes(instance.loader)) loader = instance.loader;
    return true;
  }

  function queryString({ withPage = true } = {}) {
    const q = new URLSearchParams();
    if (search.trim()) q.set('q', search.trim());
    q.set('source', source);
    // With a target instance, an explicit "any" must survive a reload, or
    // the instance's defaults would come back.
    if (gameVersion !== 'any' || instanceId) q.set('version', gameVersion);
    if (loaderApplies(safeType) && (loader !== 'any' || instanceId)) q.set('loader', loader);
    if (categories.length && source === 'modrinth') q.set('categories', categories.join(','));
    if (sort !== 'relevance') q.set('sort', sort);
    if (withPage && page > 1) q.set('page', String(page));
    if (instanceId) q.set('instance', instanceId);
    return q.toString();
  }

  /** Mirror the filters into the URL without remounting the page. */
  function syncUrl() {
    const hash = `#/discover/${safeType}?${queryString()}`;
    history.replaceState(null, '', hash);
    try {
      sessionStorage.setItem(LAST_DISCOVER_KEY, hash.slice(1));
    } catch {
      // Best effort.
    }
  }

  setHtml(
    root,
    html`
      <header class="page-head">
        <div>
          <h1>Discover</h1>
          <p>Find the next thing to add to your game.</p>
        </div>
        <div class="page-actions">
          <div class="segmented" role="group" aria-label="Result layout">
            <button data-view="grid" title="Grid view" aria-label="Grid view">${icon('grid', 'icon-sm')}</button>
            <button data-view="list" title="List view" aria-label="List view">${icon('list', 'icon-sm')}</button>
          </div>
        </div>
      </header>
      <div data-banner></div>
      <section class="card dc-search">
        <div class="row-between" style="flex-wrap:wrap">
          <div>
            <p class="eyebrow">Community content</p>
            <h2 class="dc-search-title">Explore projects</h2>
          </div>
          <div class="segmented" role="group" aria-label="Content source">
            ${SOURCES.map((option) => html`<button data-source="${option.value}">${option.label}</button>`)}
          </div>
        </div>
        <nav class="tabs" role="tablist">
          ${PROJECT_TYPES.map(
            (t) => html`<button class="tab ${t === safeType ? 'active' : ''}" role="tab" aria-selected="${t === safeType}" data-type="${t}">${projectTypeLabel(t)}s</button>`,
          )}
        </nav>
        <div class="row dc-search-row">
          <label class="input-icon grow">
            ${icon('search', 'icon-sm')}
            <input class="input" data-search type="search" value="${search}" placeholder="Search ${projectTypeLabel(safeType).toLowerCase()}s" spellcheck="false" autofocus>
          </label>
          <select class="input dc-sort" data-sort aria-label="Sort by"></select>
        </div>
      </section>
      <div class="dc-layout">
        <aside class="panel dc-filters" data-filters></aside>
        <section class="dc-results" data-results></section>
      </div>`,
  );

  const bannerEl = root.querySelector('[data-banner]');
  const filtersEl = root.querySelector('[data-filters]');
  const resultsEl = root.querySelector('[data-results]');
  const sortEl = root.querySelector('[data-sort]');
  const searchEl = root.querySelector('[data-search]');

  function drawControls() {
    root.querySelectorAll('[data-view]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.view === view)));
    root.querySelectorAll('[data-source]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.source === source)));
    setHtml(sortEl, sortsFor(source).map((s) => html`<option value="${s.value}" ${s.value === sort ? 'selected' : ''}>${s.label}</option>`));
  }

  function drawBanner() {
    if (!instanceId) return setHtml(bannerEl, '');
    const instance = targetInstance();
    if (!instance) {
      setHtml(
        bannerEl,
        store.state.instancesLoaded
          ? html`<div class="dc-banner">${icon('alert')}<span class="grow">The instance you were installing into no longer exists.</span><button class="btn btn-ghost btn-sm" data-clear-instance>${icon('x', 'icon-sm')} Clear</button></div>`
          : '',
      );
      return;
    }
    const vanillaMods = instance.loader === 'vanilla' && safeType === 'mod';
    setHtml(
      bannerEl,
      html`<div class="dc-banner">
        ${instanceIcon(instance, 36)}
        <div class="grow">
          <div>Installing into <strong>${instance.name}</strong></div>
          <div class="small muted">${instance.gameVersion} · ${loaderLabel(instance.loader)}${vanillaMods ? " — vanilla instances can't load mods; pick another type or add a loader" : ''}</div>
        </div>
        <a class="btn btn-ghost btn-sm" href="#/instance/${encodeURIComponent(instance.id)}/content">${icon('arrow-right', 'icon-sm')} Open</a>
        <button class="btn btn-outline btn-sm" data-clear-instance>${icon('x', 'icon-sm')} Clear</button>
      </div>`,
    );
  }

  function drawFilters() {
    const versions = gameVersion !== 'any' && !gameVersions.includes(gameVersion) ? [gameVersion, ...gameVersions] : gameVersions;
    const relevantCategories = categoryTags.filter((tag) => tag.projectType === categoryType(safeType));
    const groups = new Map();
    for (const tag of relevantCategories) {
      const header = tag.header || 'categories';
      if (!groups.has(header)) groups.set(header, []);
      groups.get(header).push(tag.name);
    }
    setHtml(
      filtersEl,
      html`
        <div class="row small bold">${icon('filter', 'icon-sm')} Filters</div>
        <label class="field">
          <span>Game version</span>
          <select class="input" data-filter-version>
            <option value="any">Any version</option>
            ${versions.map((v) => html`<option value="${v}" ${v === gameVersion ? 'selected' : ''}>${v}</option>`)}
          </select>
        </label>
        ${loaderApplies(safeType) &&
        html`<label class="field">
          <span>Mod loader</span>
          <select class="input" data-filter-loader>
            <option value="any">Any loader</option>
            ${KNOWN_LOADERS.map((name) => html`<option value="${name}" ${name === loader ? 'selected' : ''}>${loaderLabel(name)}</option>`)}
          </select>
        </label>`}
        ${source === 'modrinth' &&
        [...groups].map(
          ([header, names]) => html`<div class="field">
            <span>${categoryLabel(header)}</span>
            <div class="dc-chips">
              ${names.map(
                (name) => html`<button class="dc-chip" data-category="${name}" aria-pressed="${categories.includes(name)}">${categoryLabel(name)}</button>`,
              )}
            </div>
          </div>`,
        )}
        ${(gameVersion !== 'any' || loader !== 'any' || categories.length > 0) &&
        html`<button class="btn btn-ghost btn-sm" data-reset-filters>${icon('rotate-ccw', 'icon-sm')} Reset filters</button>`}
        <p class="hint dc-filters-note">Results are from ${source === 'modrinth' ? 'Modrinth' : 'CurseForge'}.${source === 'curseforge' ? ' Category filters are only available on Modrinth.' : ''}</p>`,
    );
  }

  function card(hit, index) {
    const categoriesShown = (hit.displayCategories ?? []).filter((c) => !KNOWN_LOADERS.includes(c)).slice(0, 3);
    const stats = html`
      <span title="Downloads">${icon('download', 'icon-sm')} ${compactNumber(hit.downloads)}</span>
      ${hit.follows > 0 && html`<span title="Followers">${icon('heart', 'icon-sm')} ${compactNumber(hit.follows)}</span>`}
      ${hit.dateModified && html`<span title="Updated">${icon('clock', 'icon-sm')} ${relativeTime(hit.dateModified)}</span>`}`;
    const chips = categoriesShown.map((c) => html`<span class="dc-tag">${categoryLabel(c)}</span>`);
    const installBtn = html`<button class="btn btn-outline btn-sm" data-install="${index}">${icon(hit.projectType === 'modpack' ? 'package' : 'download', 'icon-sm')} Install</button>`;

    if (view === 'list') {
      return html`<article class="dc-card dc-card-list" data-open="${index}" tabindex="0" role="link">
        ${projectIcon(hit, 64)}
        <div class="grow">
          <div class="row" style="gap:8px"><strong class="truncate dc-title">${hit.title}</strong>${projectTypeBadge(hit.projectType)}</div>
          ${hit.author && html`<div class="small muted">by ${hit.author}</div>`}
          <p class="dc-desc">${hit.description}</p>
          <div class="dc-meta">${stats}${chips}</div>
        </div>
        <div class="dc-card-side">${installBtn}</div>
      </article>`;
    }
    return html`<article class="dc-card dc-card-grid" data-open="${index}" tabindex="0" role="link">
      <div class="row" style="align-items:flex-start">
        ${projectIcon(hit, 48)}
        <div class="grow">
          <strong class="truncate dc-title" style="display:block">${hit.title}</strong>
          ${hit.author && html`<div class="tiny muted truncate">by ${hit.author}</div>`}
          <div style="margin-top:5px">${projectTypeBadge(hit.projectType)}</div>
        </div>
      </div>
      <p class="dc-desc">${hit.description}</p>
      <div class="dc-meta">${chips}</div>
      <div class="row-between dc-card-foot"><div class="dc-meta">${stats}</div>${installBtn}</div>
    </article>`;
  }

  function pageCount() {
    const capped = source === 'curseforge' ? Math.min(total, CURSEFORGE_MAX_RESULTS) : total;
    return Math.max(1, Math.ceil(capped / PAGE_SIZE));
  }

  function pagination() {
    const pages = pageCount();
    const first = total ? (page - 1) * PAGE_SIZE + 1 : 0;
    const last = Math.min(page * PAGE_SIZE, total);
    return html`<div class="dc-pager">
      <p class="small muted">Showing ${first.toLocaleString('en')}–${last.toLocaleString('en')} of ${total.toLocaleString('en')} results</p>
      ${pages > 1 &&
      html`<div class="row" style="gap:4px;flex-wrap:wrap;justify-content:center">
        <button class="btn btn-outline btn-sm" data-page="${page - 1}" ${loading || page <= 1 ? 'disabled' : ''}>${icon('chevron-left', 'icon-sm')} Previous</button>
        ${pageWindow(page, pages).map((token) =>
          token === 'gap'
            ? html`<span class="muted dc-gap">…</span>`
            : html`<button class="btn btn-sm ${token === page ? 'btn-primary' : 'btn-ghost'} dc-page-btn" data-page="${token}" ${loading || token === page ? 'disabled' : ''} aria-current="${token === page ? 'page' : 'false'}">${token}</button>`,
        )}
        <button class="btn btn-outline btn-sm" data-page="${page + 1}" ${loading || page >= pages ? 'disabled' : ''}>Next ${icon('chevron-right', 'icon-sm')}</button>
      </div>`}
    </div>`;
  }

  function drawResults() {
    const container = view === 'grid' ? 'dc-grid' : 'dc-list';
    const heading = html`<div class="row-between dc-results-head">
      <strong class="small">${loading && !hits.length ? 'Finding projects…' : `${total.toLocaleString('en')} projects`}</strong>
      ${loading && hits.length > 0 && html`<span class="tiny muted">Updating results…</span>`}
    </div>`;

    if (error && !loading) {
      setHtml(resultsEl, html`${errorBlock(error, 'dc-retry')}`);
      return;
    }
    if (loading && !hits.length) {
      setHtml(
        resultsEl,
        html`${heading}<div class="${container}">${Array.from({ length: view === 'grid' ? 8 : 6 }, () => html`<div class="skeleton ${view === 'grid' ? 'dc-skel-grid' : 'dc-skel-list'}"></div>`)}</div>`,
      );
      return;
    }
    if (!hits.length) {
      setHtml(
        resultsEl,
        html`${heading}${emptyState({
          iconName: 'compass',
          title: 'Nothing found',
          description: 'Try another search or loosen your filters.',
          actions:
            gameVersion !== 'any' || loader !== 'any' || categories.length
              ? html`<button class="btn btn-outline btn-sm" data-reset-filters>${icon('rotate-ccw', 'icon-sm')} Reset filters</button>`
              : '',
        })}`,
      );
      return;
    }
    setHtml(
      resultsEl,
      html`${heading}<div class="${container} ${loading ? 'is-loading' : ''}">${hits.map((hit, index) => card(hit, index))}</div>${pagination()}`,
    );
  }

  async function runSearch({ scroll = false } = {}) {
    const id = ++request;
    loading = true;
    error = null;
    syncUrl();
    drawResults();
    try {
      const result = await api.modrinth.search(
        {
          query: search.trim(),
          projectType: safeType,
          gameVersion: gameVersion === 'any' ? null : gameVersion,
          loader: loaderApplies(safeType) && loader !== 'any' ? loader : null,
          categories: source === 'modrinth' ? categories : [],
          index: sort,
        },
        PAGE_SIZE,
        (page - 1) * PAGE_SIZE,
        source,
      );
      if (disposed || id !== request) return;
      hits = result.hits ?? [];
      total = result.totalHits ?? 0;
      // A reload on a page that no longer exists (fewer results now) lands on the last one.
      if (!hits.length && page > 1 && total > 0) {
        page = pageCount();
        return runSearch({ scroll });
      }
    } catch (cause) {
      if (disposed || id !== request) return;
      error = cause;
      hits = [];
      total = 0;
    }
    loading = false;
    drawResults();
    if (scroll) resultsEl.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  const refilter = () => {
    page = 1;
    drawFilters();
    runSearch();
  };

  const debouncedSearch = debounce(() => {
    if (disposed) return;
    page = 1;
    runSearch();
  }, 350);

  searchEl.addEventListener('input', () => {
    search = searchEl.value;
    debouncedSearch();
  });

  root.addEventListener('change', (event) => {
    const target = event.target;
    if (target === sortEl) {
      sort = sortEl.value;
      page = 1;
      return runSearch();
    }
    if (target.matches('[data-filter-version]')) {
      gameVersion = target.value;
      return refilter();
    }
    if (target.matches('[data-filter-loader]')) {
      loader = target.value;
      return refilter();
    }
  });

  const openHit = (index) => {
    const hit = hits[index];
    if (!hit) return;
    try {
      if (hit.author) sessionStorage.setItem(`minearcade.author.${hit.projectId}`, hit.author);
    } catch {
      // Best effort.
    }
    navigate(projectPath(hit.projectId, instanceId));
  };

  root.addEventListener('click', (event) => {
    const target = event.target;
    const install = target.closest('[data-install]');
    if (install) {
      const hit = hits[Number(install.dataset.install)];
      if (hit) openInstallDialog(hit, { instanceId: instanceId || undefined });
      return;
    }
    const viewBtn = target.closest('[data-view]');
    if (viewBtn) {
      view = viewBtn.dataset.view === 'list' ? 'list' : 'grid';
      prefs.set('discover.view', view);
      drawControls();
      return drawResults();
    }
    const sourceBtn = target.closest('[data-source]');
    if (sourceBtn) {
      if (sourceBtn.dataset.source === source) return;
      source = sourceBtn.dataset.source;
      prefs.set('discover.source', source);
      if (!sortsFor(source).some((s) => s.value === sort)) sort = 'relevance';
      hits = [];
      drawControls();
      return refilter();
    }
    const typeBtn = target.closest('[data-type]');
    if (typeBtn) {
      if (typeBtn.dataset.type === safeType) return;
      prefs.set('discover.type', typeBtn.dataset.type);
      categories = [];
      page = 1;
      return navigate(`/discover/${typeBtn.dataset.type}?${queryString({ withPage: false })}`);
    }
    const chip = target.closest('[data-category]');
    if (chip) {
      const name = chip.dataset.category;
      categories = categories.includes(name) ? categories.filter((c) => c !== name) : [...categories, name];
      return refilter();
    }
    if (target.closest('[data-reset-filters]')) {
      gameVersion = 'any';
      loader = 'any';
      categories = [];
      return refilter();
    }
    if (target.closest('[data-clear-instance]')) {
      instanceId = '';
      drawBanner();
      return syncUrl();
    }
    const pageBtn = target.closest('[data-page]');
    if (pageBtn && !pageBtn.disabled) {
      page = Math.min(Math.max(1, Number(pageBtn.dataset.page)), pageCount());
      return runSearch({ scroll: true });
    }
    if (target.closest('#dc-retry')) return runSearch();
    const cardEl = target.closest('[data-open]');
    if (cardEl && !target.closest('a, button, select, input')) openHit(Number(cardEl.dataset.open));
  });

  root.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    const cardEl = event.target.closest?.('[data-open]');
    if (cardEl && event.target === cardEl) {
      event.preventDefault();
      openHit(Number(cardEl.dataset.open));
    }
  });

  const off = store.on(['instances', 'instancesLoaded'], () => {
    drawBanner();
    if (applyInstanceDefaults()) refilter();
  });

  applyInstanceDefaults();
  drawControls();
  drawBanner();
  drawFilters();
  runSearch();

  api.modrinth
    .tags('game_version')
    .then((tags) => {
      if (disposed) return;
      const releases = tags.filter((tag) => tag.versionType === 'release');
      gameVersions = (releases.length ? releases : tags).slice(0, MAX_VERSIONS).map((tag) => tag.name);
      drawFilters();
    })
    .catch((cause) => !disposed && toastError(cause, "Couldn't load game versions"));

  api.modrinth
    .tags('category')
    .then((tags) => {
      if (disposed) return;
      categoryTags = tags;
      drawFilters();
    })
    .catch(() => {
      // Categories are optional filters; search still works without them.
    });

  return () => {
    disposed = true;
    off();
  };
}
