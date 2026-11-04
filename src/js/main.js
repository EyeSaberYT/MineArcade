// App bootstrap: the top bar, backend events, progress footer, routes.
import * as api from './api.js';
import {
  loadSettings,
  refreshAccounts,
  refreshInstances,
  refreshRunning,
  stopInstance,
  wasStopped,
} from './actions.js';
import { mountAccountMenu } from './components/account-menu.js';
import { initDrop } from './drop.js';
import { currentRoute, navigate, onNavigate, route, startRouter } from './router.js';
import { appendConsole, store } from './store.js';
import { errorText, listen } from './tauri.js';
import { $, $$, html, icon, initTooltips, raw, setHtml, spinner, toast } from './ui.js';
import { startUpdateCheck } from './updater.js';

// ---------- Routes ----------

route('/', () => import('./pages/home.js'));
route('/library', () => import('./pages/library.js'));
route('/instance/:id/:tab?', () => import('./pages/instance.js'));
route('/discover/:type?', () => import('./pages/discover.js'));
route('/project/:source/:id', () => import('./pages/project.js'));
route('/settings/:section?', () => import('./pages/settings.js'));

// ---------- Shell: the top bar's tabs and status ----------

const sectionOf = (path) => {
  if (path === '/' || path === '') return 'home';
  if (path.startsWith('/library') || path.startsWith('/instance')) return 'library';
  if (path.startsWith('/discover') || path.startsWith('/project')) return 'discover';
  if (path.startsWith('/settings')) return 'settings';
  return '';
};

/** The tabs: the current section lights up. */
function mountNav() {
  const nav = $('#nav');
  const highlight = (path) => {
    const section = sectionOf(path);
    $$('.nav-tab', nav).forEach((tab) => tab.classList.toggle('active', tab.dataset.section === section));
  };
  onNavigate(highlight);
  highlight(currentRoute().path);
  mountAccountMenu($('#account-box'));
}

/**
 * The status chips on the right of the bar: whether the launcher is online, the Orbit Client
 * version it plays, and the running game (with stop and logs) or "Idle".
 */
function mountTopBar() {
  const back = $('#nav-back');
  const statusEl = $('#topbar-actions');
  let orbitVersion = null;

  back.addEventListener('click', () => history.back());
  // Back only matters away from the tabs' own pages: an instance or a project.
  const renderBack = (path) => {
    back.hidden = !(path.startsWith('/instance/') || path.startsWith('/project/'));
  };

  const render = () => {
    const running = [...store.state.running];
    const first = running.length ? store.state.instances.find((i) => i.id === running[0]) : null;
    const games = [...store.state.runningCounts.values()].reduce((sum, n) => sum + n, 0);
    setHtml(
      statusEl,
      html`
        <span class="chip ${navigator.onLine ? '' : 'is-off'}"><span class="chip-dot"></span><b>${navigator.onLine ? 'Online' : 'Offline'}</b></span>
        ${orbitVersion && html`<span class="chip" data-tip="Orbit Client version">ORBIT <b>${orbitVersion}</b></span>`}
        ${running.length
          ? html`<span class="chip is-live">
              <span class="chip-dot"></span>
              <a href="#/instance/${encodeURIComponent(running[0])}" data-tip="View instance">${first?.name ?? 'Minecraft'}</a>
              ${games > 1 && html`<span class="faint">+${games - 1}</span>`}
              <button class="chip-btn danger" data-stop-running data-tip="Stop game" aria-label="Stop game">${icon('stop-circle', 'icon-sm')}</button>
              <button class="chip-btn" data-logs data-tip="View logs" aria-label="View logs">${icon('terminal', 'icon-sm')}</button>
            </span>`
          : html`<span class="chip">STATUS <b>Idle</b></span>`}`,
    );
  };

  const refreshOrbit = async () => {
    try {
      orbitVersion = (await api.orbit.status())?.version ?? null;
    } catch {
      orbitVersion = null;
    }
    render();
  };

  statusEl.addEventListener('click', (event) => {
    const running = [...store.state.running];
    if (event.target.closest('[data-stop-running]') && running.length) {
      const instance = store.state.instances.find((i) => i.id === running[0]);
      return stopInstance(running[0], instance?.name);
    }
    if (event.target.closest('[data-logs]') && running.length) navigate(`/instance/${running[0]}/console`);
  });

  window.addEventListener('online', render);
  window.addEventListener('offline', render);
  onNavigate(renderBack);
  store.on(['running', 'runningCounts', 'instances'], render);
  // A launch of Orbit Client may have updated its jar.
  store.on('instances', refreshOrbit);
  renderBack(currentRoute().path);
  refreshOrbit();
}

// ---------- Window controls ----------

/**
 * The window is undecorated on Windows and Linux, so the top bar draws its own
 * minimize / maximize / close (like the Modrinth App). macOS keeps its native
 * traffic lights over the bar, so there the bar only leaves room for them.
 */
function mountWindowControls() {
  const appWindow = window.__TAURI__?.window?.getCurrentWindow?.();
  if (!appWindow) return; // A plain browser (dev preview): nothing to control.
  if (/Mac/i.test(navigator.platform || navigator.userAgent)) {
    document.documentElement.classList.add('is-mac');
    return;
  }

  const box = $('#window-controls');
  const glyphs = {
    minimize: '<path d="M1 5.5h9"/>',
    maximize: '<rect x="1.5" y="1.5" width="8" height="8" rx="1"/>',
    restore: '<rect x="1.5" y="3.5" width="6" height="6" rx="1"/><path d="M3.5 3.5v-1a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-1"/>',
    close: '<path d="m1.5 1.5 8 8M9.5 1.5l-8 8"/>',
  };
  const glyph = (name) => raw(`<svg class="window-glyph" viewBox="0 0 11 11" aria-hidden="true">${glyphs[name]}</svg>`);

  const render = (maximized) => {
    setHtml(
      box,
      html`
        <button class="window-btn" data-window="minimize" aria-label="Minimize">${glyph('minimize')}</button>
        <button class="window-btn" data-window="maximize" aria-label="${maximized ? 'Restore' : 'Maximize'}">${glyph(maximized ? 'restore' : 'maximize')}</button>
        <button class="window-btn is-close" data-window="close" aria-label="Close">${glyph('close')}</button>`,
    );
  };

  box.addEventListener('click', (event) => {
    const action = event.target.closest('[data-window]')?.dataset.window;
    if (action === 'minimize') appWindow.minimize();
    if (action === 'maximize') appWindow.toggleMaximize();
    if (action === 'close') appWindow.close();
  });

  // Keep the maximize/restore glyph honest when the window is snapped or double-clicked.
  const sync = async () => render(await appWindow.isMaximized().catch(() => false));
  appWindow.onResized?.(sync);
  box.hidden = false;
  sync();
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
    // The backend has dropped this game already; other copies of the instance may still run.
    await refreshRunning();
    const name = store.state.instances.find((i) => i.id === instanceId)?.name ?? 'Minecraft';
    if (wasStopped(instanceId)) {
      toast(`${name} stopped`);
    } else if (code === 0 || code === null) {
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
  mountNav();
  mountTopBar();
  mountWindowControls();
  mountProgressFooter();
  initDrop();
  await listenToBackend();
  await Promise.all([refreshInstances(), refreshAccounts(), refreshRunning()]);
  startRouter();
  startUpdateCheck();
}

boot().catch((error) => {
  console.error(error);
  toast('Orbit Launcher failed to start', { kind: 'error', description: errorText(error), duration: 0 });
});
