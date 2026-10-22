// Settings, laid out and behaving like Aether's: an overlay over the current
// screen with seven panels. Edits collect in a draft that the "Save changes"
// footer commits, while the theme and accent colour apply (and save) immediately.
import * as api from '../api.js';
import { applyAccent, ACCENTS, currentAccent, saveSettings } from '../actions.js';
import { navigate } from '../router.js';
import { store } from '../store.js';
import { formatBytes, html, icon, openModal, setHtml, spinner, toast, toastError } from '../ui.js';
import { checkForUpdates, installUpdate, relaunchApp } from '../updater.js';

const JAVA_MAJORS = [8, 16, 17, 21, 25];
const RELEASES_URL = 'https://github.com/EyeSaberYT/MineArcade/releases/latest';
const DEFAULT_SERVER = 'play.minearcade.org';

const PANELS = [
  { id: 'appearance', label: 'Appearance', icon: 'sun', help: 'Theme and display' },
  { id: 'general', label: 'General', icon: 'settings', help: 'Launcher behavior' },
  { id: 'java', label: 'Java', icon: 'cpu', help: 'Runtime management' },
  { id: 'memory', label: 'Memory & JVM', icon: 'monitor', help: 'Default resources' },
  { id: 'storage', label: 'Storage', icon: 'folder', help: 'Files and cache' },
  { id: 'updates', label: 'Updates', icon: 'refresh', help: 'Version status' },
  { id: 'about', label: 'About', icon: 'info', help: 'App information' },
];

let runtimes = [];
let installMajor = '21';
let installing = false;
let openDialog = null;

/** Open Settings as an overlay over whatever screen is showing. */
export function openSettings(initialPanel = 'general') {
  if (openDialog && !openDialog.closed) return;
  let cleanup = () => {};
  openDialog = openModal({ size: 'xl', onClose: () => cleanup() });
  openDialog.root.classList.add('settings-dialog');
  cleanup = mount(openDialog.body, PANELS.some((p) => p.id === initialPanel) ? initialPanel : 'general', openDialog.close);
}

/** `#/settings/:section` links open the overlay on top of Home. */
export function render(_root, params) {
  queueMicrotask(() => {
    navigate('/', { replace: true });
    openSettings(params.section);
  });
}

function mount(root, initialPanel, close) {
  let panel = initialPanel;
  let disposed = false;
  let saving = false;
  // Like Aether, the draft starts from (and follows) the saved settings.
  let draft = store.state.settings ? { ...store.state.settings } : null;

  const draw = () => {
    if (disposed) return;
    if (!store.state.settings || !draft) {
      setHtml(root, html`<div class="settings-popup settings-loading">Loading launcher preferences…</div>`);
      return;
    }
    setHtml(
      root,
      html`
        <section class="settings-popup" aria-label="MineArcade settings">
          <header class="settings-popup-header">
            <div class="grow">
              <p class="eyebrow" style="color:var(--primary)">Launcher preferences</p>
              <h1>Settings</h1>
              <p class="small muted">Configure MineArcade for your setup.</p>
            </div>
            <button class="btn btn-ghost btn-icon" data-close-settings aria-label="Close settings">${icon('x')}</button>
          </header>
          <div class="settings-layout">
            <nav class="settings-nav" role="tablist" aria-orientation="vertical" aria-label="Settings categories">
              ${PANELS.map(
                (p) => html`<button class="settings-nav-item ${p.id === panel ? 'active' : ''}" role="tab" aria-selected="${p.id === panel}" data-panel="${p.id}">
                  ${icon(p.icon)}<span class="grow"><b>${p.label}</b><small>${p.help}</small></span>
                </button>`,
              )}
            </nav>
            <main class="settings-pane" role="tabpanel">${PANEL_RENDERERS[panel]()}</main>
          </div>
          <footer class="settings-popup-footer">
            <button class="btn btn-primary" data-save ${saving ? 'disabled' : ''}>${saving ? spinner() : icon('save')} Save changes</button>
            <p class="small muted">Settings are saved on this device.</p>
          </footer>
        </section>`,
    );
  };

  const section = (iconName, title, subtitle, body) => html`
    <section class="settings-section">
      <header class="settings-section-head">
        <span class="settings-section-icon">${icon(iconName)}</span>
        <div><h2>${title}</h2><p class="small muted">${subtitle}</p></div>
      </header>
      <div>${body}</div>
    </section>`;

  const toggle = (key, label, description) => html`
    <label class="settings-row switch-row">
      <span class="grow"><b>${label}</b><small>${description}</small></span>
      <input type="checkbox" data-key="${key}" data-type="bool" ${draft[key] ? 'checked' : ''}>
      <span class="switch"></span>
    </label>`;

  const PANEL_RENDERERS = {
    appearance: () => {
      const accent = currentAccent();
      return section(
        'sun',
        'Appearance',
        'Tune how the launcher looks.',
        html`
          <div class="theme-choices">
            ${['dark', 'light', 'system'].map(
              (theme) => html`<button class="theme-choice ${draft.theme === theme ? 'active' : ''}" data-theme-choice="${theme}">
                <span class="theme-swatch is-${theme}"></span><span style="text-transform:capitalize">${theme}</span>
              </button>`,
            )}
          </div>
          <div style="margin-top:28px">
            <h3 style="font-size:14px">Accent color</h3>
            <p class="small muted" style="margin:4px 0 12px">Choose the highlight color used across MineArcade.</p>
            <div class="accent-choices" role="radiogroup" aria-label="Accent color">
              ${ACCENTS.map(
                (a) => html`<button class="accent-choice ${accent.id === a.id ? 'active' : ''}" role="radio" aria-checked="${accent.id === a.id}" data-accent="${a.id}">
                  <span class="accent-dot" style="background:${a.color}"></span><span>${a.label}</span>
                </button>`,
              )}
            </div>
            <label class="accent-custom ${accent.id === 'custom' ? 'active' : ''}">
              <span class="accent-dot" style="background:${accent.custom}"></span>
              <span class="grow">Custom color</span>
              <span class="mono tiny muted">${accent.custom.toUpperCase()}</span>
              <input type="color" data-custom-accent value="${accent.custom}" aria-label="Choose a custom accent color">
            </label>
          </div>`,
      );
    },

    general: () =>
      section(
        'settings',
        'General',
        'Launcher behavior and downloads.',
        html`
          <div class="settings-rows">
            ${toggle('showSnapshots', 'Show snapshots and betas', 'Include unreleased Minecraft versions in version pickers.')}
            ${toggle('checkUpdatesOnStart', 'Check for updates on launch', 'Look for a newer MineArcade release when the launcher starts.')}
            ${toggle('closeOnLaunch', 'Close launcher when game starts', 'Quit MineArcade as soon as Minecraft opens.')}
            ${toggle('keepLauncherOpen', 'Keep launcher open', 'Keep MineArcade available while Minecraft is running. When off, it minimizes instead.')}
            <div class="settings-row">
              <span class="grow"><b>Concurrent downloads</b><small>How many files to fetch in parallel.</small></span>
              <input class="input settings-num" type="number" min="1" max="32" data-key="concurrentDownloads" data-type="int" value="${draft.concurrentDownloads}">
            </div>
            <div class="settings-row">
              <span class="grow"><b>MineArcade server</b><small>Used by the Join buttons and added to every new instance's server list.</small></span>
              <input class="input mono settings-server" data-key="featuredServer" value="${draft.featuredServer ?? ''}" placeholder="${DEFAULT_SERVER}" spellcheck="false">
            </div>
          </div>`,
      ),

    java: () =>
      section(
        'cpu',
        'Java',
        'MineArcade can manage the right runtime for each Minecraft version.',
        html`
          <div class="settings-java-grid">
            <div class="stack">
              <label class="field">
                <span>Java executable</span>
                <input class="input" data-key="javaPath" data-type="path" value="${draft.javaPath ?? ''}" placeholder="Auto-detect" spellcheck="false">
                <span class="hint">Leave blank to auto-detect per game version.</span>
              </label>
              <div>
                <div class="row-between" style="margin-bottom:8px">
                  <span class="field-label">Installed runtimes</span>
                  <button class="btn btn-ghost btn-sm" data-java-refresh>${icon('refresh', 'icon-sm')} Refresh</button>
                </div>
                ${runtimes.length === 0
                  ? html`<p class="settings-empty">No Java runtimes detected. MineArcade will download one when a game needs it.</p>`
                  : html`<ul class="runtime-list">${runtimes.map(
                      (runtime) => html`<li class="runtime-row">
                        <span style="color:var(--primary)">${icon('terminal', 'icon-sm')}</span>
                        <div class="grow">
                          <div class="truncate bold small">Java ${runtime.major} · ${runtime.version}</div>
                          <div class="truncate tiny muted" title="${runtime.path}">${runtime.path}</div>
                        </div>
                        <span class="badge">${runtime.vendor || runtime.source}</span>
                      </li>`,
                    )}</ul>`}
              </div>
            </div>
            <div class="runtime-install-card">
              <span class="settings-section-icon">${icon('download', 'icon-sm')}</span>
              <h3 style="margin-top:12px;font-size:14px">Add a runtime</h3>
              <p class="tiny muted" style="margin-top:4px;line-height:1.5">Download a supported Java runtime directly into MineArcade.</p>
              <select class="input" style="margin-top:14px" data-java-major>
                ${JAVA_MAJORS.map((major) => html`<option value="${major}" ${String(major) === installMajor ? 'selected' : ''}>Java ${major}</option>`)}
              </select>
              <button class="btn btn-outline btn-block" style="margin-top:8px" data-java-install ${installing ? 'disabled' : ''}>
                ${installing ? spinner() : icon('cpu')} Install Java
              </button>
            </div>
          </div>`,
      ),

    memory: () =>
      section(
        'monitor',
        'Memory & JVM',
        'Default resource limits for newly created instances.',
        html`
          <div class="grid-2">
            <label class="field"><span>Minimum memory (MB)</span>
              <input class="input" type="number" data-key="defaultMemoryMin" data-type="int" value="${draft.defaultMemoryMin}"></label>
            <label class="field"><span>Maximum memory (MB)</span>
              <input class="input" type="number" data-key="defaultMemoryMax" data-type="int" value="${draft.defaultMemoryMax}"></label>
          </div>`,
      ),

    storage: () => {
      const info = store.state.info;
      const rows = [
        ['Data directory', info?.dataDir],
        ['Instances', info?.instancesDir],
        ['Java runtimes', info?.javaDir],
        ['Cache', info?.cacheDir],
      ].filter(([, path]) => path);
      return section(
        'folder',
        'Storage',
        'MineArcade’s local files and cache locations.',
        html`<div class="settings-rows">${rows.map(
          ([label, path]) => html`<div class="settings-row">
            <span class="grow"><b>${label}</b><small class="truncate" title="${path}">${path}</small></span>
            <button class="btn btn-ghost btn-icon btn-sm" data-reveal="${path}" aria-label="Open ${label}">${icon('folder', 'icon-sm')}</button>
          </div>`,
        )}</div>`,
      );
    },

    updates: () => {
      const { status, update, progress, error } = store.state.updater;
      const version = store.state.info?.version ?? 'unknown';
      let body;
      if (status === 'downloading' && progress) {
        const percent = progress.total ? Math.round((progress.downloaded / progress.total) * 100) : null;
        body = html`<div class="stack-sm">
          <div class="row-between small">
            <span>Downloading MineArcade ${update?.version ?? ''}</span>
            <span class="muted">${progress.total ? `${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}` : formatBytes(progress.downloaded)}</span>
          </div>
          <div class="progress-track"><div class="progress-fill ${percent === null ? 'indeterminate' : ''}" style="${percent !== null ? `width:${percent}%` : ''}"></div></div>
        </div>`;
      } else if (status === 'installed') {
        body = html`<div class="row-between" style="flex-wrap:wrap">
          <p class="small">MineArcade ${update?.version ?? ''} is installed. Restart to finish updating.</p>
          <button class="btn btn-primary" data-restart>${icon('rotate-ccw')} Restart now</button>
        </div>`;
      } else if (status === 'available' && update) {
        body = html`<div class="row-between" style="flex-wrap:wrap">
          <p class="small">MineArcade ${update.version} is ready to install.</p>
          <button class="btn btn-primary" data-install-update>${icon('download')} Update now</button>
        </div>`;
      } else {
        const message =
          status === 'checking'
            ? 'Checking for updates…'
            : status === 'up-to-date'
              ? `You're on the latest version (${version}).`
              : status === 'error'
                ? (error ?? 'Update check failed.')
                : 'Check for the latest MineArcade release.';
        body = html`<div class="row-between" style="flex-wrap:wrap">
          <p class="small muted">${message}</p>
          <button class="btn btn-outline" data-check-updates ${status === 'checking' ? 'disabled' : ''}>
            ${status === 'checking' ? spinner() : icon('refresh')} Check for updates
          </button>
        </div>`;
      }
      return section(
        'refresh',
        'Updates',
        'Keep MineArcade up to date.',
        html`${body}<p class="tiny muted" style="margin-top:16px">Need an installer? <a data-open-url="${RELEASES_URL}">Browse releases ${icon('external', 'icon-sm')}</a></p>`,
      );
    },

    about: () => {
      const info = store.state.info;
      return section(
        'info',
        'About MineArcade',
        'Free, ad-free, and built for the MineArcade server.',
        html`
          ${info
            ? html`<div class="grid-3">${[
                ['Version', info.version],
                ['Platform', `${info.os} · ${info.arch}`],
                ['Application', info.name],
              ].map(([label, value]) => html`<div class="about-tile"><small>${label}</small><strong>${value}</strong></div>`)}</div>`
            : html`<p class="small muted">Loading application information…</p>`}
          <p class="tiny muted" style="margin-top:16px;line-height:1.6">Downloaded game assets and metadata are cached under the data directory listed in Storage. MineArcade is MIT-licensed.</p>`,
      );
    },
  };

  // ----- actions -----

  async function persist(patch) {
    saving = true;
    draw();
    try {
      await saveSettings(patch ?? draft);
      toast('Settings saved', { kind: 'success' });
    } catch (error) {
      toastError(error);
    } finally {
      saving = false;
    }
    draw();
  }

  async function refreshJava() {
    try {
      runtimes = await api.java.list();
    } catch (error) {
      toastError(error);
    }
    draw();
  }

  async function installJava() {
    installing = true;
    draw();
    const toastId = toast(`Downloading Java ${installMajor}…`, { kind: 'loading' });
    try {
      const runtime = await api.java.install(Number(installMajor));
      toast(`Java ${runtime.major} installed`, { id: toastId, kind: 'success' });
      runtimes = await api.java.list().catch(() => runtimes);
    } catch (error) {
      toast(String(error), { id: toastId, kind: 'error' });
    } finally {
      installing = false;
    }
    draw();
  }

  // ----- events -----

  root.addEventListener('input', (event) => {
    const input = event.target;
    if (input.matches('[data-custom-accent]')) return applyAccent('custom', input.value);
    if (!input.dataset.key) return;
    const { key, type } = input.dataset;
    if (type === 'bool') draft[key] = input.checked;
    else if (type === 'int') draft[key] = Number(input.value) || (key === 'concurrentDownloads' ? 1 : 0);
    else if (type === 'path') draft[key] = input.value || null;
    else draft[key] = input.value;
  });

  root.addEventListener('change', (event) => {
    if (event.target.matches('[data-java-major]')) installMajor = event.target.value;
    if (event.target.matches('[data-custom-accent]')) draw();
  });

  root.addEventListener('click', (event) => {
    const target = event.target;
    const nav = target.closest('[data-panel]');
    if (nav) {
      panel = nav.dataset.panel;
      if (panel === 'java') refreshJava();
      return draw();
    }
    if (target.closest('[data-close-settings]')) return close();
    if (target.closest('[data-save]')) return persist();

    const theme = target.closest('[data-theme-choice]');
    if (theme) {
      draft.theme = theme.dataset.themeChoice;
      return persist({ theme: draft.theme });
    }
    const accent = target.closest('[data-accent]');
    if (accent) {
      applyAccent(accent.dataset.accent);
      return draw();
    }

    if (target.closest('[data-java-refresh]')) return refreshJava();
    if (target.closest('[data-java-install]')) return installJava();
    if (target.closest('[data-check-updates]')) return checkForUpdates();
    if (target.closest('[data-install-update]')) return installUpdate();
    if (target.closest('[data-restart]')) return relaunchApp();

    const reveal = target.closest('[data-reveal]');
    if (reveal) return api.system.reveal(reveal.dataset.reveal).catch(toastError);
    const url = target.closest('[data-open-url]');
    if (url) {
      event.preventDefault();
      return api.system.openUrl(url.dataset.openUrl).catch(toastError);
    }
  });

  const offSettings = store.on(['settings', 'info'], () => {
    draft = store.state.settings ? { ...store.state.settings } : null;
    draw();
  });
  const offUpdater = store.on('updater', () => panel === 'updates' && draw());

  draw();
  if (panel === 'java') refreshJava();

  return () => {
    disposed = true;
    offSettings();
    offUpdater();
  };
}
