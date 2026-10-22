// Settings: launcher behaviour, the MineArcade server, game defaults, Java,
// appearance, updates and app information. Edits collect in a draft that a
// Save bar commits; the theme applies (and saves) immediately.
import * as api from '../api.js';
import { saveSettings } from '../actions.js';
import { currentRoute, navigate } from '../router.js';
import { store } from '../store.js';
import { pickFile } from '../tauri.js';
import {
  emptyState,
  errorBlock,
  formatBytes,
  formatDate,
  formatMemory,
  html,
  icon,
  loadingBlock,
  setHtml,
  spinner,
  toast,
  toastError,
} from '../ui.js';
import { checkForUpdates, installUpdate, relaunchApp } from '../updater.js';

const DEFAULT_SERVER = 'play.minearcade.org';
const WEBSITE_URL = 'https://minearcade.org';
const RELEASES_URL = 'https://github.com/EyeSaberYT/MineArcade/releases/latest';
const JAVA_MAJORS = [8, 16, 17, 21, 25];

const SECTIONS = [
  { id: 'general', label: 'General', icon: 'settings', help: 'Launcher & server' },
  { id: 'game', label: 'Memory & JVM', icon: 'cpu', help: 'Game defaults' },
  { id: 'java', label: 'Java', icon: 'coffee', help: 'Runtime management' },
  { id: 'appearance', label: 'Appearance', icon: 'sun', help: 'Theme' },
  { id: 'updates', label: 'Updates', icon: 'refresh', help: 'Version status' },
  { id: 'about', label: 'About', icon: 'info', help: 'App & folders' },
];

const THEMES = [
  { id: 'dark', label: 'Dark', icon: 'moon' },
  { id: 'light', label: 'Light', icon: 'sun' },
  { id: 'system', label: 'System', icon: 'monitor' },
];

// Kept at module level so switching sections (which re-renders the page)
// doesn't lose unsaved edits or in-flight work.
let edits = {};
let javaRuntimes = null;
let javaError = null;
let javaLoading = false;
let installingMajor = null;
let installMajor = 21;

// ---------- JVM argument text <-> list ----------

/** Split a command line into arguments, honouring "double" and 'single' quotes. */
function parseArgs(text) {
  const args = [];
  let current = '';
  let quote = null;
  let started = false;
  for (const char of String(text)) {
    if (quote) {
      if (char === quote) quote = null;
      else current += char;
    } else if (char === '"' || char === "'") {
      quote = char;
      started = true;
    } else if (/\s/.test(char)) {
      if (started) args.push(current);
      current = '';
      started = false;
    } else {
      current += char;
      started = true;
    }
  }
  if (started) args.push(current);
  return args;
}

const joinArgs = (args) => (args ?? []).map((arg) => (/[\s"']/.test(arg) || !arg ? `"${arg}"` : arg)).join(' ');

// ---------- Draft ----------

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** Saved settings with the unsaved edits on top. */
const draft = () => ({ ...store.state.settings, ...edits });

function setEdit(key, value) {
  const saved = store.state.settings?.[key];
  if (same(saved, value)) delete edits[key];
  else edits[key] = value;
}

function validate(values) {
  const errors = {};
  const { defaultMemoryMin: min, defaultMemoryMax: max, concurrentDownloads: downloads } = values;
  if (!Number.isInteger(min) || min < 256) errors.defaultMemoryMin = 'Use a whole number of at least 256 MB.';
  if (!Number.isInteger(max) || max < 512) errors.defaultMemoryMax = 'Use a whole number of at least 512 MB.';
  else if (!errors.defaultMemoryMin && min > max) errors.defaultMemoryMax = 'Maximum memory must be at least the minimum.';
  if (!Number.isInteger(downloads) || downloads < 1 || downloads > 32) {
    errors.concurrentDownloads = 'Pick a number from 1 to 32.';
  }
  const server = (values.featuredServer ?? '').trim();
  if (server && !/^[A-Za-z0-9._\-[\]]+(?::\d{1,5})?$/.test(server)) {
    errors.featuredServer = 'Enter a host name such as play.minearcade.org, optionally with :port.';
  }
  return errors;
}

// ---------- Page ----------

export function render(root, params) {
  const section = params.section;
  if (!SECTIONS.some((s) => s.id === section)) {
    queueMicrotask(() => navigate('/settings/general', { replace: true }));
    return;
  }

  let disposed = false;
  let saving = false;

  setHtml(
    root,
    html`
      <header class="page-head">
        <div>
          <div class="eyebrow">Launcher preferences</div>
          <h1>Settings</h1>
          <p>Configure MineArcade for your setup.</p>
        </div>
      </header>
      <div class="settings-layout">
        <nav class="settings-nav" role="tablist" aria-label="Settings categories">
          ${SECTIONS.map(
            (s) => html`
              <button class="settings-nav-item ${s.id === section ? 'active' : ''}" role="tab" aria-selected="${s.id === section}" data-section="${s.id}">
                ${icon(s.icon)}
                <span class="grow"><b>${s.label}</b><small>${s.help}</small></span>
              </button>`,
          )}
        </nav>
        <div class="settings-pane stack-lg" data-pane></div>
      </div>
      <div class="settings-savebar" data-savebar hidden></div>`,
  );

  const pane = root.querySelector('[data-pane]');
  const saveBar = root.querySelector('[data-savebar]');

  // ----- rendering -----

  function draw() {
    if (disposed) return;
    const settings = store.state.settings;
    if (!settings) {
      setHtml(pane, loadingBlock('Loading launcher preferences…'));
      return;
    }
    const values = draft();
    const renderers = { general, game, java, appearance, updates, about };
    setHtml(pane, renderers[section](values));
    showErrors();
    drawSaveBar();
  }

  function drawSaveBar() {
    const dirty = Object.keys(edits).length > 0;
    saveBar.hidden = !dirty;
    if (!dirty) return;
    const errors = validate(draft());
    const invalid = Object.keys(errors).length > 0;
    setHtml(
      saveBar,
      html`
        <span class="row grow">
          ${icon(invalid ? 'alert' : 'info', 'icon-sm')}
          <span>${invalid ? 'Fix the highlighted fields to save.' : 'You have unsaved changes.'}</span>
        </span>
        <button class="btn btn-ghost btn-sm" data-discard ${saving ? 'disabled' : ''}>Discard</button>
        <button class="btn btn-primary btn-sm" data-save ${invalid || saving ? 'disabled' : ''}>
          ${saving ? spinner() : icon('save', 'icon-sm')} Save changes
        </button>`,
    );
  }

  function showErrors() {
    const errors = validate(draft());
    for (const node of pane.querySelectorAll('[data-error-for]')) {
      node.textContent = errors[node.dataset.errorFor] ?? '';
    }
    for (const input of pane.querySelectorAll('[data-key]')) {
      input.classList.toggle('is-invalid', Boolean(errors[input.dataset.key]));
    }
  }

  const field = (label, control, { hint, key } = {}) => html`
    <div class="field">
      <span>${label}</span>
      ${control}
      ${hint && html`<span class="hint">${hint}</span>`}
      ${key && html`<span class="form-error" data-error-for="${key}"></span>`}
    </div>`;

  const toggle = (key, label, description, checked) => html`
    <label class="switch-row settings-row">
      <span class="grow"><b>${label}</b><span class="hint">${description}</span></span>
      <input type="checkbox" data-key="${key}" data-type="bool" ${checked ? 'checked' : ''}>
      <span class="switch"></span>
    </label>`;

  const sectionHead = (iconName, title, subtitle) => html`
    <div class="settings-section-head">
      <span class="settings-section-icon">${icon(iconName)}</span>
      <div><h2>${title}</h2><p class="hint">${subtitle}</p></div>
    </div>`;

  function general(values) {
    const server = values.featuredServer ?? '';
    return html`
      <section class="panel">
        ${sectionHead('settings', 'General', 'Launcher behaviour and downloads.')}
        <div class="settings-rows">
          <div class="settings-row">
            <span class="grow"><b>When a game starts</b><span class="hint">Minimizing gets MineArcade out of the way; it stays one click away in the taskbar.</span></span>
            <div class="segmented">
              <button data-launch-behaviour="keep" aria-pressed="${!values.closeOnLaunch}">Keep open</button>
              <button data-launch-behaviour="minimize" aria-pressed="${Boolean(values.closeOnLaunch)}">Minimize</button>
            </div>
          </div>
          ${toggle('showSnapshots', 'Show snapshots and betas', 'Include unreleased Minecraft versions in version pickers.', values.showSnapshots)}
          <div class="settings-row">
            <span class="grow"><b>Concurrent downloads</b><span class="hint">How many files to fetch in parallel (1–32).</span>
              <span class="form-error" data-error-for="concurrentDownloads"></span></span>
            <input class="input settings-num" type="number" min="1" max="32" step="1" data-key="concurrentDownloads" data-type="int" value="${values.concurrentDownloads}">
          </div>
        </div>
      </section>
      <section class="panel">
        ${sectionHead('server', 'MineArcade server', 'The server MineArcade is built around.')}
        <div class="stack">
          ${field(
            'Server address',
            html`<div class="row">
              <input class="input mono" data-key="featuredServer" data-type="text" value="${server}" placeholder="${DEFAULT_SERVER}" spellcheck="false">
              <button class="btn btn-outline" data-reset-server ${server.trim() === DEFAULT_SERVER ? 'disabled' : ''}>${icon('rotate-ccw', 'icon-sm')} Reset to default</button>
            </div>`,
            {
              key: 'featuredServer',
              hint: 'Powers the Join buttons across MineArcade and is added to the multiplayer list of every new instance. Leave it empty to hide the Join buttons.',
            },
          )}
        </div>
      </section>`;
  }

  function game(values) {
    const memoryHint = (mb) => (Number.isFinite(mb) && mb > 0 ? `≈ ${formatMemory(mb)}` : '');
    return html`
      <section class="panel">
        ${sectionHead('cpu', 'Memory', 'Default resource limits for newly created and imported instances.')}
        <div class="grid-2">
          ${field(
            html`Minimum memory <em>(MB)</em> <b data-memory-hint="defaultMemoryMin">${memoryHint(values.defaultMemoryMin)}</b>`,
            html`<input class="input" type="number" min="256" step="256" data-key="defaultMemoryMin" data-type="int" value="${values.defaultMemoryMin}">`,
            { key: 'defaultMemoryMin' },
          )}
          ${field(
            html`Maximum memory <em>(MB)</em> <b data-memory-hint="defaultMemoryMax">${memoryHint(values.defaultMemoryMax)}</b>`,
            html`<input class="input" type="number" min="512" step="256" data-key="defaultMemoryMax" data-type="int" value="${values.defaultMemoryMax}">`,
            { key: 'defaultMemoryMax' },
          )}
        </div>
        <p class="hint" style="margin-top:10px">Existing instances keep their own memory settings; change them from each instance’s Settings tab.</p>
      </section>
      <section class="panel">
        ${sectionHead('terminal', 'JVM arguments', 'Extra Java options for every game launch.')}
        ${field(
          'Default JVM arguments',
          html`<textarea class="input mono" rows="3" data-key="defaultJvmArgs" data-type="args" spellcheck="false" placeholder="-Dfile.encoding=UTF-8">${joinArgs(values.defaultJvmArgs)}</textarea>`,
          { hint: 'Separated by spaces; quote an argument that contains spaces. These come before each instance’s own arguments.' },
        )}
      </section>`;
  }

  function java(values) {
    let list;
    if (javaRuntimes === null && javaError) list = errorBlock(javaError, 'java-retry');
    else if (javaRuntimes === null) list = loadingBlock('Looking for Java runtimes…');
    else if (!javaRuntimes.length) {
      list = emptyState({
        iconName: 'coffee',
        title: 'No Java runtimes detected',
        description: 'MineArcade downloads the right one automatically when a game needs it.',
      });
    } else {
      list = html`<div class="list">
        ${javaRuntimes.map(
          (runtime) => html`
            <div class="list-row">
              <span class="settings-section-icon">${icon('terminal', 'icon-sm')}</span>
              <div class="grow">
                <div class="truncate bold">Java ${runtime.major} <span class="muted small">· ${runtime.version}</span></div>
                <div class="truncate small muted mono selectable" title="${runtime.path}">${runtime.path}</div>
              </div>
              <span class="badge">${runtime.vendor || runtime.source}</span>
              ${values.javaPath === runtime.path
                ? html`<span class="badge badge-mod">${icon('check', 'icon-sm')} In use</span>`
                : html`<button class="btn btn-ghost btn-sm" data-use-java="${runtime.path}">Use</button>`}
            </div>`,
        )}
      </div>`;
    }

    return html`
      <section class="panel">
        ${sectionHead('coffee', 'Java', 'MineArcade picks the right runtime for each Minecraft version automatically.')}
        ${field(
          'Java executable override',
          html`<div class="row">
            <input class="input mono" data-key="javaPath" data-type="path" value="${values.javaPath ?? ''}" placeholder="Auto-detect" spellcheck="false">
            <button class="btn btn-outline" data-browse-java>${icon('folder')} Browse…</button>
            ${values.javaPath && html`<button class="btn btn-ghost" data-clear-java>${icon('x', 'icon-sm')} Clear</button>`}
          </div>`,
          { hint: 'Leave blank to auto-detect per game version. An instance’s own Java setting still wins over this one.' },
        )}
      </section>
      <div class="settings-java-grid">
        <section class="stack">
          <div class="row-between">
            <span class="field-label">Installed runtimes</span>
            <button class="btn btn-ghost btn-sm" data-java-refresh ${javaLoading ? 'disabled' : ''}>
              ${javaLoading ? spinner('icon-sm') : icon('refresh', 'icon-sm')} Refresh
            </button>
          </div>
          ${list}
        </section>
        <section class="panel stack">
          <span class="settings-section-icon">${icon('download')}</span>
          <div>
            <h3>Add a runtime</h3>
            <p class="hint">Download a Mojang Java runtime straight into MineArcade.</p>
          </div>
          <select class="input" data-java-major ${installingMajor ? 'disabled' : ''}>
            ${JAVA_MAJORS.map((major) => html`<option value="${major}" ${major === installMajor ? 'selected' : ''}>Java ${major}</option>`)}
          </select>
          <button class="btn btn-outline" data-java-install ${installingMajor ? 'disabled' : ''}>
            ${installingMajor ? html`${spinner()} Installing Java ${installingMajor}…` : html`${icon('download')} Install Java`}
          </button>
        </section>
      </div>`;
  }

  function appearance() {
    // The saved theme, not the draft: theme changes save on the spot.
    const theme = store.state.settings.theme;
    return html`
      <section class="panel">
        ${sectionHead('sun', 'Appearance', 'Tune how the launcher looks.')}
        <div class="settings-themes" role="radiogroup" aria-label="Theme">
          ${THEMES.map(
            (t) => html`
              <button class="settings-theme ${theme === t.id ? 'active' : ''}" role="radio" aria-checked="${theme === t.id}" data-theme-choice="${t.id}">
                <span class="settings-theme-swatch is-${t.id}"><i></i><i></i><i></i></span>
                <span class="row">${icon(t.icon, 'icon-sm')} ${t.label}</span>
              </button>`,
          )}
        </div>
        <p class="hint" style="margin-top:12px">System follows your operating system’s light or dark mode.</p>
      </section>`;
  }

  function updates(values) {
    const { status, update, progress, error } = store.state.updater;
    const version = store.state.info?.version ?? 'unknown';
    let body;
    if (status === 'downloading') {
      const total = progress?.total;
      const downloaded = progress?.downloaded ?? 0;
      const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      body = html`
        <div class="stack-sm">
          <div class="row-between">
            <span class="row">${spinner()} Downloading MineArcade ${update?.version ?? ''}</span>
            <span class="small muted tabular">${total ? `${formatBytes(downloaded)} / ${formatBytes(total)}` : formatBytes(downloaded)}</span>
          </div>
          <div class="progress-track"><div class="progress-fill ${percent === null ? 'indeterminate' : ''}" style="${percent !== null ? `width:${percent}%` : ''}"></div></div>
        </div>`;
    } else if (status === 'installed') {
      body = html`
        <div class="row-between">
          <span class="row">${icon('check-circle', 'success-text')} MineArcade ${update?.version ?? ''} is installed. Restart to finish updating.</span>
          <button class="btn btn-primary" data-restart>${icon('rotate-ccw')} Restart now</button>
        </div>`;
    } else if (status === 'available' && update) {
      const released = update.date ? formatDate(update.date) : '—';
      body = html`
        <div class="stack">
          <div class="row-between">
            <span>
              <span class="row">${icon('arrow-up-circle')} <b>MineArcade ${update.version} is available</b></span>
              <span class="hint">You’re running ${update.currentVersion ?? version}${released !== '—' ? ` · released ${released}` : ''}.</span>
            </span>
            <button class="btn btn-primary" data-install-update>${icon('download')} Update now</button>
          </div>
          ${update.body?.trim() &&
          html`<div class="stack-sm"><span class="field-label">Release notes</span><div class="settings-notes selectable">${update.body.trim()}</div></div>`}
        </div>`;
    } else {
      const message =
        status === 'checking'
          ? html`<span class="row muted">${spinner()} Checking for updates…</span>`
          : status === 'up-to-date'
            ? html`<span class="row">${icon('check-circle', 'success-text')} You’re on the latest version (${version}).</span>`
            : status === 'error'
              ? html`<span class="row danger-text">${icon('alert')} <span class="selectable">${error ?? 'The update check failed.'}</span></span>`
              : html`<span class="muted">Check for the latest MineArcade release.</span>`;
      body = html`
        <div class="row-between">
          ${message}
          <button class="btn btn-outline" data-check-updates ${status === 'checking' ? 'disabled' : ''}>
            ${status === 'checking' ? spinner() : icon('refresh')} Check for updates
          </button>
        </div>`;
    }

    return html`
      <section class="panel">
        ${sectionHead('refresh', 'Updates', 'Keep MineArcade up to date.')}
        <div class="settings-version"><small>Current version</small><strong>${version}</strong></div>
        <div style="margin-top:14px">${body}</div>
        <p class="hint" style="margin-top:14px">Need an installer? <a data-open-url="${RELEASES_URL}">Browse releases ${icon('external', 'icon-sm')}</a></p>
      </section>
      <section class="panel settings-rows">
        ${toggle('checkUpdatesOnStart', 'Check for updates on startup', 'Look for a new MineArcade release a few seconds after the launcher opens.', values.checkUpdatesOnStart)}
      </section>`;
  }

  function about() {
    const info = store.state.info;
    if (!info) return loadingBlock('Loading application information…');
    const folders = [
      ['Data directory', info.dataDir],
      ['Instances', info.instancesDir],
      ['Java runtimes', info.javaDir],
      ['Cache', info.cacheDir],
    ].filter(([, path]) => path);
    return html`
      <section class="panel">
        ${sectionHead('info', 'About MineArcade', 'A Minecraft launcher built for the MineArcade server.')}
        <div class="grid-3">
          <div class="settings-version"><small>Application</small><strong>${info.name}</strong></div>
          <div class="settings-version"><small>Version</small><strong>${info.version}</strong></div>
          <div class="settings-version"><small>Platform</small><strong>${info.os} · ${info.arch}</strong></div>
        </div>
        <div class="row-wrap" style="margin-top:14px">
          <button class="btn btn-cyan btn-sm" data-open-url="${WEBSITE_URL}">${icon('globe', 'icon-sm')} minearcade.org</button>
          <button class="btn btn-ghost btn-sm" data-open-url="${RELEASES_URL}">${icon('external', 'icon-sm')} Release notes</button>
        </div>
      </section>
      <section class="panel">
        ${sectionHead('hard-drive', 'Storage', 'Where MineArcade keeps its files.')}
        <div class="list">
          ${folders.map(
            ([label, path]) => html`
              <div class="list-row">
                <div class="grow">
                  <div class="bold">${label}</div>
                  <div class="truncate small muted mono selectable" title="${path}">${path}</div>
                </div>
                <button class="btn btn-ghost btn-sm" data-reveal="${path}" aria-label="Open ${label}">${icon('folder', 'icon-sm')} Open</button>
              </div>`,
          )}
        </div>
        <p class="hint" style="margin-top:10px">Downloaded game assets and metadata are cached under the data directory.</p>
      </section>`;
  }

  // ----- Java -----

  async function loadJava() {
    javaLoading = true;
    javaError = null;
    if (section === 'java') draw();
    try {
      javaRuntimes = await api.java.list();
    } catch (error) {
      javaError = error;
    } finally {
      javaLoading = false;
    }
    if (!disposed && section === 'java') draw();
  }

  async function installJava() {
    if (installingMajor) return;
    const major = installMajor;
    installingMajor = major;
    draw();
    const toastId = toast(`Downloading Java ${major}…`, { kind: 'loading' });
    try {
      const runtime = await api.java.install(major);
      toast(`Java ${runtime.major} installed`, { id: toastId, kind: 'success' });
      javaRuntimes = await api.java.list().catch(() => javaRuntimes);
    } catch (error) {
      toast(`Java ${major} couldn’t be installed`, { id: toastId, kind: 'error', description: String(error) });
    } finally {
      installingMajor = null;
    }
    if (!disposed) draw();
  }

  // ----- saving -----

  async function save() {
    if (saving || !Object.keys(edits).length) return;
    if (Object.keys(validate(draft())).length) return;
    saving = true;
    drawSaveBar();
    const patch = { ...edits };
    if (typeof patch.featuredServer === 'string') patch.featuredServer = patch.featuredServer.trim();
    if (typeof patch.javaPath === 'string') patch.javaPath = patch.javaPath.trim() || null;
    if ('closeOnLaunch' in patch) patch.keepLauncherOpen = !patch.closeOnLaunch;
    try {
      await saveSettings(patch);
      edits = {};
      toast('Settings saved', { kind: 'success', duration: 2000 });
    } catch (error) {
      toastError(error, "Settings couldn't be saved");
    } finally {
      saving = false;
    }
    if (!disposed) draw();
  }

  // ----- events -----

  function onInput(event) {
    const input = event.target.closest('[data-key]');
    if (!input) return;
    const { key, type } = input.dataset;
    let value;
    if (type === 'bool') value = input.checked;
    else if (type === 'int') value = input.value.trim() === '' ? NaN : Number(input.value);
    else if (type === 'args') value = parseArgs(input.value);
    else if (type === 'path') value = input.value.trim() ? input.value : null;
    else value = input.value;
    setEdit(key, value);

    const hint = pane.querySelector(`[data-memory-hint="${key}"]`);
    if (hint) hint.textContent = Number.isFinite(value) && value > 0 ? `≈ ${formatMemory(value)}` : '';
    if (key === 'featuredServer') {
      const reset = pane.querySelector('[data-reset-server]');
      if (reset) reset.disabled = input.value.trim() === DEFAULT_SERVER;
    }
    showErrors();
    drawSaveBar();
  }
  root.addEventListener('input', onInput);
  root.addEventListener('change', (event) => {
    if (event.target.matches('[data-java-major]')) installMajor = Number(event.target.value);
  });

  root.addEventListener('keydown', (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      save();
    }
  });

  root.addEventListener('click', async (event) => {
    const target = event.target;
    const nav = target.closest('[data-section]');
    if (nav) {
      if (nav.dataset.section !== section) navigate(`/settings/${nav.dataset.section}`, { replace: true });
      return;
    }
    if (target.closest('[data-save]')) return save();
    if (target.closest('[data-discard]')) {
      edits = {};
      return draw();
    }

    const behaviour = target.closest('[data-launch-behaviour]');
    if (behaviour) {
      setEdit('closeOnLaunch', behaviour.dataset.launchBehaviour === 'minimize');
      return draw();
    }
    if (target.closest('[data-reset-server]')) {
      setEdit('featuredServer', DEFAULT_SERVER);
      return draw();
    }

    const themeChoice = target.closest('[data-theme-choice]');
    if (themeChoice) {
      const theme = themeChoice.dataset.themeChoice;
      if (theme === store.state.settings?.theme) return;
      try {
        await saveSettings({ theme });
        toast('Settings saved', { kind: 'success', duration: 2000 });
      } catch (error) {
        toastError(error, "The theme couldn't be saved");
      }
      return;
    }

    if (target.closest('[data-browse-java]')) {
      try {
        const path = await pickFile({ title: 'Choose a Java executable (java or javaw)' });
        if (path && !disposed) {
          setEdit('javaPath', path);
          draw();
        }
      } catch (error) {
        toastError(error);
      }
      return;
    }
    if (target.closest('[data-clear-java]')) {
      setEdit('javaPath', null);
      return draw();
    }
    const useJava = target.closest('[data-use-java]');
    if (useJava) {
      setEdit('javaPath', useJava.dataset.useJava);
      return draw();
    }
    if (target.closest('[data-java-refresh]') || target.closest('#java-retry')) return loadJava();
    if (target.closest('[data-java-install]')) return installJava();

    if (target.closest('[data-check-updates]')) return checkForUpdates();
    if (target.closest('[data-install-update]')) return installUpdate();
    if (target.closest('[data-restart]')) return relaunchApp();

    const url = target.closest('[data-open-url]');
    if (url) {
      event.preventDefault();
      return api.system.openUrl(url.dataset.openUrl).catch(toastError);
    }
    const reveal = target.closest('[data-reveal]');
    if (reveal) return api.system.reveal(reveal.dataset.reveal).catch(toastError);
  });

  // ----- store -----

  const offSettings = store.on(['settings', 'info'], () => {
    // Drop edits that now match what's saved (e.g. after a save elsewhere).
    for (const key of Object.keys(edits)) setEdit(key, edits[key]);
    draw();
  });
  const offUpdater = store.on('updater', () => section === 'updates' && draw());

  draw();
  if (section === 'java' && javaRuntimes === null && !javaLoading) loadJava();

  return () => {
    disposed = true;
    offSettings();
    offUpdater();
    // Unsaved edits survive switching sections, not leaving Settings.
    if (!currentRoute().path.startsWith('/settings') && Object.keys(edits).length) {
      edits = {};
      toast('Unsaved settings were discarded', { kind: 'warn', duration: 3000 });
    }
  };
}
