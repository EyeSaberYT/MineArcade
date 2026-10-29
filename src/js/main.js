// App bootstrap: the rail and top bar, backend events, progress footer, routes.
import * as api from './api.js';
import {
  featuredServer,
  launchInstance,
  loadSettings,
  quickJoin,
  refreshAccounts,
  refreshInstances,
  refreshRunning,
  spotlightInstance,
  stopInstance,
} from './actions.js';
import { mountAccountMenu } from './components/account-menu.js';
import { initDrop } from './drop.js';
import { revealInstanceFolder } from './pages/instance/shared.js';
import { currentCrumb, currentRoute, navigate, onCrumb, onNavigate, route, startRouter } from './router.js';
import { appendConsole, store } from './store.js';
import { errorText, listen } from './tauri.js';
import { $, $$, html, icon, initTooltips, instanceIcon, setHtml, showMenu, spinner, toast } from './ui.js';
import { startUpdateCheck } from './updater.js';

// ---------- Routes ----------

route('/', () => import('./pages/home.js'));
route('/library', () => import('./pages/library.js'));
route('/instance/:id/:tab?', () => import('./pages/instance.js'));
route('/discover/:type?', () => import('./pages/discover.js'));
route('/project/:source/:id', () => import('./pages/project.js'));
route('/settings/:section?', () => import('./pages/settings.js'));

// ---------- Shell: rail and top bar ----------

const sectionOf = (path) => {
  if (path === '/' || path === '') return 'home';
  if (path.startsWith('/library') || path.startsWith('/instance')) return 'library';
  if (path.startsWith('/discover') || path.startsWith('/project')) return 'discover';
  return '';
};

const openSettings = async () => (await import('./pages/settings.js')).openSettings();
const openNewInstance = async () => (await import('./dialogs/new-instance.js')).openNewInstanceDialog();

/** Most recently played first; never-played instances last, by name. */
function recentInstances() {
  return [...store.state.instances].sort((a, b) => {
    const diff = new Date(b.lastPlayed ?? 0) - new Date(a.lastPlayed ?? 0);
    return diff || a.name.localeCompare(b.name);
  });
}

function mountRail() {
  const nav = $('#nav');
  const recentEl = $('#rail-recent');
  let path = currentRoute().path;
  // As many recent instances as fit between the main buttons and the footer.
  let fits = 0;

  const highlight = () => {
    const section = sectionOf(path);
    $$('.rail-btn', nav).forEach((item) => item.classList.toggle('active', item.dataset.section === section));
  };

  const renderRecent = () => {
    const shown = recentInstances().slice(0, fits);
    const openId = path.startsWith('/instance/') ? decodeURIComponent(path.split('/')[2] ?? '') : null;
    setHtml(
      recentEl,
      shown.length
        ? html`<div class="rail-divider"></div>
            ${shown.map(
              (instance) => html`<a class="rail-btn rail-instance ${instance.id === openId ? 'active' : ''}" href="#/instance/${encodeURIComponent(instance.id)}"
                  data-recent="${instance.id}" data-tip="${instance.name}" aria-label="${instance.name}">
                ${instanceIcon(instance, 30)}
                ${store.state.running.has(instance.id) && html`<span class="rail-live" aria-label="Running"></span>`}
              </a>`,
            )}
            <div class="rail-divider"></div>`
        : '',
    );
  };

  // Each slot is a 48px button plus a 4px gap; the two dividers take 34px.
  new ResizeObserver(() => {
    const next = Math.max(0, Math.floor((recentEl.clientHeight - 34) / 52));
    if (next !== fits) {
      fits = next;
      // Next frame, so redrawing can't re-trigger the observer mid-callback.
      requestAnimationFrame(renderRecent);
    }
  }).observe(recentEl);

  // Right-click a recent instance for quick actions, as in the Modrinth App.
  recentEl.addEventListener('contextmenu', (event) => {
    const node = event.target.closest('[data-recent]');
    const instance = node && store.state.instances.find((i) => i.id === node.dataset.recent);
    if (!instance) return;
    event.preventDefault();
    const running = store.state.running.has(instance.id);
    showMenu(
      node,
      [
        { header: instance.name },
        running
          ? { label: 'Stop', icon: 'stop-circle', danger: true, onSelect: () => stopInstance(instance.id, instance.name) }
          : { label: 'Play', icon: 'play', disabled: store.state.launching.has(instance.id), onSelect: () => launchInstance(instance.id) },
        { separator: true },
        { label: 'View instance', icon: 'eye', onSelect: () => navigate(`/instance/${instance.id}`) },
        { label: 'Open folder', icon: 'folder', onSelect: () => revealInstanceFolder(instance.id) },
      ],
      { placement: 'right' },
    );
  });

  $('[data-new-instance]').addEventListener('click', openNewInstance);
  $('[data-open-settings]').addEventListener('click', openSettings);

  onNavigate((next) => {
    path = next;
    highlight();
    renderRecent();
  });
  store.on(['instances', 'running'], renderRecent);
  highlight();

  mountAccountMenu($('#account-box'));
}

function mountTopBar() {
  const back = $('#nav-back');
  const forward = $('#nav-forward');
  const crumbsEl = $('#crumbs');
  const actionsEl = $('#topbar-actions');
  let path = currentRoute().path;

  back.addEventListener('click', () => history.back());
  forward.addEventListener('click', () => history.forward());

  // The Navigation API (WebView2) knows whether there is anywhere to go; elsewhere both stay enabled.
  const renderArrows = () => {
    const navigation = window.navigation;
    back.disabled = navigation ? !navigation.canGoBack : false;
    forward.disabled = navigation ? !navigation.canGoForward : false;
  };

  const crumbsFor = () => {
    if (path.startsWith('/instance/')) {
      const id = decodeURIComponent(path.split('/')[2] ?? '');
      const name = store.state.instances.find((i) => i.id === id)?.name ?? 'Instance';
      return [{ label: 'Library', href: '#/library' }, { label: name }];
    }
    if (path.startsWith('/project/')) {
      // Back to the search the project was opened from (pages/discover.js keeps it here).
      let search = '/discover';
      try {
        search = sessionStorage.getItem('minearcade.discover.last') || search;
      } catch {
        // Plain Discover then.
      }
      return [{ label: 'Discover content', href: `#${search}` }, { label: currentCrumb() ?? 'Project' }];
    }
    const labels = { home: 'Home', library: 'Library', discover: 'Discover content' };
    return [{ label: labels[sectionOf(path)] ?? 'MineArcade' }];
  };

  const renderCrumbs = () => {
    const crumbs = crumbsFor();
    setHtml(
      crumbsEl,
      crumbs.map((crumb, index) =>
        index < crumbs.length - 1
          ? html`<a class="crumb" href="${crumb.href}">${crumb.label}</a><span class="crumb-sep" aria-hidden="true">${icon('chevron-right', 'icon-sm')}</span>`
          : html`<span class="crumb crumb-current" aria-current="page">${crumb.label}</span>`,
      ),
    );
  };

  // Right side: the MineArcade join button (Home has its own) and the running-game pill.
  const renderActions = () => {
    const address = featuredServer();
    const spotlight = spotlightInstance();
    const running = [...store.state.running];
    const launching = spotlight && store.state.launching.has(spotlight.id);
    const showJoin = address && sectionOf(path) !== 'home' && !(spotlight && store.state.running.has(spotlight.id));
    const first = running.length ? store.state.instances.find((i) => i.id === running[0]) : null;

    setHtml(
      actionsEl,
      html`
        ${showJoin &&
        html`<button class="btn btn-primary btn-sm" data-join data-tip="Join ${address}${spotlight ? ` with ${spotlight.name}` : ''}" ${launching ? 'disabled' : ''}>
          ${launching ? spinner() : icon('server', 'icon-sm')} ${launching ? 'Starting…' : 'Join MineArcade'}
        </button>`}
        ${running.length
          ? html`<div class="run-pill">
              <span class="live-dot"></span>
              <a href="#/instance/${encodeURIComponent(running[0])}" data-tip="View instance">${first?.name ?? 'Minecraft'}</a>
              ${running.length > 1 && html`<span class="faint small">+${running.length - 1}</span>`}
              <button class="run-pill-btn danger" data-stop-running data-tip="Stop instance" aria-label="Stop instance">${icon('stop-circle')}</button>
              <button class="run-pill-btn" data-logs data-tip="View logs" aria-label="View logs">${icon('terminal')}</button>
            </div>`
          : html`<div class="run-pill is-idle"><span class="idle-dot"></span>No instances running</div>`}`,
    );
  };

  actionsEl.addEventListener('click', (event) => {
    const running = [...store.state.running];
    if (event.target.closest('[data-join]')) return quickJoin();
    if (event.target.closest('[data-stop-running]') && running.length) {
      const instance = store.state.instances.find((i) => i.id === running[0]);
      return stopInstance(running[0], instance?.name);
    }
    if (event.target.closest('[data-logs]') && running.length) navigate(`/instance/${running[0]}/console`);
  });

  onNavigate((next) => {
    path = next;
    renderArrows();
    renderCrumbs();
    renderActions();
  });
  onCrumb(renderCrumbs);
  store.on('instances', renderCrumbs);
  store.on(['settings', 'instances', 'running', 'launching'], renderActions);
  renderArrows();
  renderCrumbs();
  renderActions();
}

// ---------- Progress footer ----------

function mountProgressFooter() {
  const footer = $('#progress-footer');
  let hideTimer;

  const render = () => {
    const p = store.state.progress;
    clearTimeout(hideTimer);
    if (!p) {
      footer.hidden = true;
      return;
    }
    const hasBytes = p.total > 0;
    const hasFiles = (p.filesTotal ?? 0) > 0;
    const percent = p.finished
      ? 100
      : hasBytes
        ? Math.min(100, Math.round((p.current / p.total) * 100))
        : hasFiles
          ? Math.min(100, Math.round(((p.filesDone ?? 0) / p.filesTotal) * 100))
          : null;

    footer.hidden = false;
    footer.classList.toggle('is-done', Boolean(p.finished));
    setHtml(
      footer,
      html`
        ${p.finished ? html`<span style="color:var(--green)">${icon('check-circle')}</span>` : spinner()}
        <div class="grow">
          <div class="row-between">
            <span class="label truncate">${p.label}</span>
            <span class="small muted nowrap tabular">
              ${p.finished ? 'Done' : percent !== null ? `${percent}%` : ''}
              ${!p.finished && (p.filesTotal ?? 0) > 1 ? ` · ${p.filesDone ?? 0}/${p.filesTotal} files` : ''}
            </span>
          </div>
          ${!p.finished &&
          html`<div class="progress-track" style="margin-top:7px;height:6px">
            <div class="progress-fill ${percent === null ? 'indeterminate' : ''}" style="${percent !== null ? `width:${percent}%` : ''}"></div>
          </div>`}
        </div>
        <button class="btn btn-ghost btn-icon btn-sm" data-dismiss aria-label="Dismiss">${icon('x')}</button>`,
    );
    if (p.finished) hideTimer = setTimeout(() => store.set('progress', null), 4000);
  };

  footer.addEventListener('click', (event) => {
    if (event.target.closest('[data-dismiss]')) store.set('progress', null);
  });
  store.on('progress', render);
  render();
}

// ---------- Backend events ----------

async function listenToBackend() {
  // Progress arrives very often during downloads: render at most once a frame.
  let pending = null;
  await listen('download://progress', (progress) => {
    const first = pending === null;
    pending = progress;
    if (first) {
      requestAnimationFrame(() => {
        store.set('progress', pending);
        pending = null;
      });
    }
  });

  await listen('auth://device-code', (info) => store.set('deviceCode', info));

  await listen('game://output', ({ instanceId, stream, line }) => appendConsole(instanceId, stream, line));

  await listen('game://exit', async ({ instanceId, code }) => {
    await api.launch.markStopped(instanceId).catch(() => {});
    store.update('running', (set) => void set.delete(instanceId));
    const name = store.state.instances.find((i) => i.id === instanceId)?.name ?? 'Minecraft';
    if (code === 0 || code === null) {
      toast(`${name} closed`);
    } else {
      toast(`${name} exited with code ${code}`, {
        kind: 'error',
        description: 'Open the instance’s Console tab to see the last lines it printed.',
        actions: [{ label: 'Open console', onClick: () => (location.hash = `#/instance/${instanceId}/console`) }],
      });
    }
    refreshInstances();
  });
}

// ---------- Boot ----------

async function boot() {
  try {
    await loadSettings();
  } catch (error) {
    toast("Couldn't load settings", { kind: 'error', description: errorText(error) });
  }
  initTooltips();
  mountRail();
  mountTopBar();
  mountProgressFooter();
  initDrop();
  await listenToBackend();
  await Promise.all([refreshInstances(), refreshAccounts(), refreshRunning()]);
  startRouter();
  startUpdateCheck();
}

boot().catch((error) => {
  console.error(error);
  toast('MineArcade failed to start', { kind: 'error', description: errorText(error), duration: 0 });
});
