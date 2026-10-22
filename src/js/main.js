// App bootstrap: sidebar, backend events, progress footer, routes.
import * as api from './api.js';
import {
  featuredServer,
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
import { currentRoute, onNavigate, route, startRouter } from './router.js';
import { appendConsole, store } from './store.js';
import { errorText, listen } from './tauri.js';
import { $, $$, html, icon, setHtml, spinner, toast } from './ui.js';
import { startUpdateCheck } from './updater.js';

// ---------- Routes ----------

route('/', () => import('./pages/home.js'));
route('/library', () => import('./pages/library.js'));
route('/instance/:id/:tab?', () => import('./pages/instance.js'));
route('/discover/:type?', () => import('./pages/discover.js'));
route('/project/:source/:id', () => import('./pages/project.js'));
route('/settings/:section?', () => import('./pages/settings.js'));

// ---------- Sidebar ----------

function mountSidebar() {
  const nav = $('#nav');
  const sectionOf = (path) => {
    if (path === '/' || path === '') return 'home';
    if (path.startsWith('/library') || path.startsWith('/instance')) return 'library';
    if (path.startsWith('/discover') || path.startsWith('/project')) return 'discover';
    if (path.startsWith('/settings')) return 'settings';
    return '';
  };
  const highlight = (path) => {
    const section = sectionOf(path);
    $$('.nav-item', nav).forEach((item) => item.classList.toggle('active', item.dataset.section === section));
  };
  onNavigate(highlight);
  highlight(currentRoute().path);

  // Settings is an overlay (as in Aether), not a page.
  nav.querySelector('[data-open-settings]').addEventListener('click', async (event) => {
    event.preventDefault();
    (await import('./pages/settings.js')).openSettings();
  });

  const badge = $('#running-badge');
  const renderBadge = () => {
    const count = store.state.running.size;
    badge.hidden = count === 0;
    badge.textContent = String(count);
  };
  store.on('running', renderBadge);
  renderBadge();

  // Featured server card: one click joins with the most recent instance.
  const serverBox = $('#sidebar-server');
  const renderServer = () => {
    const address = featuredServer();
    serverBox.hidden = !address;
    if (!address) return;
    const instance = spotlightInstance();
    const running = instance && store.state.running.has(instance.id);
    const launching = instance && store.state.launching.has(instance.id);
    setHtml(
      serverBox,
      html`
        <div class="row" style="gap:7px">${icon('server', 'icon-sm')}<span class="bold small">MineArcade</span></div>
        <div class="addr" title="${address}">${address}</div>
        ${running
          ? html`<button class="btn btn-danger btn-sm btn-block" data-stop>${icon('square')} Stop game</button>`
          : html`<button class="btn btn-play btn-sm btn-block" data-join ${launching ? 'disabled' : ''}>
              ${launching ? spinner() : icon('play')} ${launching ? 'Starting…' : 'Join server'}
            </button>`}`,
    );
  };
  serverBox.addEventListener('click', (event) => {
    if (event.target.closest('[data-join]')) quickJoin();
    if (event.target.closest('[data-stop]')) {
      const instance = spotlightInstance();
      if (instance) stopInstance(instance.id, instance.name);
    }
  });
  store.on(['settings', 'instances', 'running', 'launching'], renderServer);
  renderServer();

  mountAccountMenu($('#account-box'));
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
  mountSidebar();
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
