const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const MAX_LOG_LINES = 3000;

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
const esc = (value) =>
  String(value ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

const ICONS = {
  edit: '<svg viewBox="0 0 24 24"><path d="M4 17.2V20h2.8l8.3-8.3-2.8-2.8zM19.7 7.3a1 1 0 0 0 0-1.4l-1.6-1.6a1 1 0 0 0-1.4 0l-1.3 1.3 2.8 2.8z"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8z"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M9 3h6l1 2h4v2H4V5h4zM6 9h12l-1 12H7z"/></svg>',
};

const state = {
  info: null,
  settings: null,
  instances: [],
  accounts: { active: null, accounts: [] },
  manifest: null,
  fabricCache: new Map(),
  running: new Set(),
  launching: new Set(),
  progress: {}, // instanceId -> { stage, message, current, total }
  logs: {}, // instanceId -> [{ level, line }]
};

// ---------- Helpers ----------

const fmtMem = (mb) => {
  mb = Number(mb);
  return mb >= 1024 ? `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB` : `${mb} MB`;
};

const fmtAgo = (secs) => {
  if (!secs) return 'Never played';
  const diff = Date.now() / 1000 - secs;
  if (diff < 60) return 'Played just now';
  if (diff < 3600) return `Played ${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `Played ${Math.floor(diff / 3600)} h ago`;
  const days = Math.floor(diff / 86400);
  return `Played ${days} day${days === 1 ? '' : 's'} ago`;
};

const loaderLabel = (inst) => (inst.loader === 'fabric' ? `Fabric ${inst.loaderVersion ?? ''}`.trim() : 'Vanilla');

const hueOf = (text) => {
  let hue = 0;
  for (const c of text) hue = (hue * 31 + c.charCodeAt(0)) % 360;
  return hue;
};

const avatarUrl = (uuid, size = 64) => `https://mc-heads.net/avatar/${encodeURIComponent(uuid)}/${size}`;

function hideBrokenAvatars(root) {
  $$('img.avatar', root).forEach((img) => img.addEventListener('error', () => img.classList.add('broken'), { once: true }));
}

function selectedInstance() {
  const id = state.settings?.selectedInstance;
  return state.instances.find((i) => i.id === id) ?? state.instances[0] ?? null;
}

async function getManifest() {
  state.manifest ??= await invoke('get_versions');
  return state.manifest;
}

async function getFabricVersions(mcVersion) {
  if (!state.fabricCache.has(mcVersion)) {
    state.fabricCache.set(mcVersion, await invoke('fabric_versions', { mcVersion }));
  }
  return state.fabricCache.get(mcVersion);
}

// ---------- Toasts ----------

function toast(message, kind = 'info', ms = 4500) {
  const el = document.createElement('div');
  el.className = `toast toast-${kind}`;
  el.textContent = message;
  $('#toasts').appendChild(el);
  requestAnimationFrame(() => el.classList.add('show'));
  setTimeout(() => {
    el.classList.remove('show');
    setTimeout(() => el.remove(), 250);
  }, ms);
}

// ---------- Modal ----------

let modalToken = 0;
let modalOnClose = null;

function openModal(html, { onClose = null } = {}) {
  if (modalOnClose) {
    const previous = modalOnClose;
    modalOnClose = null;
    previous();
  }
  modalToken++;
  const body = document.createElement('div');
  body.innerHTML = html;
  $('#modal-body').replaceChildren(body);
  $('#modal').hidden = false;
  modalOnClose = onClose;
  hideBrokenAvatars(body);
  body.querySelector('input:not([type=checkbox]):not([type=range])')?.focus();
  return body;
}

function closeModal({ silent = false } = {}) {
  modalToken++;
  $('#modal').hidden = true;
  $('#modal-body').replaceChildren();
  const callback = modalOnClose;
  modalOnClose = null;
  if (callback && !silent) callback();
}

function confirmDialog({ title, message, confirm = 'Confirm', danger = false }) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (!settled) {
        settled = true;
        resolve(value);
      }
    };
    const body = openModal(
      `<h2>${esc(title)}</h2>
       <p class="muted">${esc(message)}</p>
       <div class="modal-actions">
         <button class="btn btn-ghost" data-close>Cancel</button>
         <button class="btn ${danger ? 'btn-danger' : 'btn-primary'}" id="confirm-ok">${esc(confirm)}</button>
       </div>`,
      { onClose: () => finish(false) },
    );
    $('#confirm-ok', body).addEventListener('click', () => {
      finish(true);
      closeModal({ silent: true });
    });
  });
}

// ---------- Navigation ----------

function showPage(name) {
  $$('.nav-item').forEach((b) => b.classList.toggle('active', b.dataset.page === name));
  $$('.page').forEach((p) => p.classList.toggle('active', p.id === `page-${name}`));
  if (name === 'settings') renderSettings();
}

// ---------- Rendering ----------

function render() {
  renderAccount();
  renderPlay();
  renderInstances();
}

function renderAccount() {
  const box = $('#account-box');
  const active = state.accounts.accounts.find((a) => a.uuid === state.accounts.active);
  if (!active) {
    box.innerHTML = `<button class="btn btn-primary btn-block" id="signin-btn">Sign in with Microsoft</button>`;
    $('#signin-btn').addEventListener('click', startLogin);
    return;
  }
  box.innerHTML = `
    <button class="account-card" id="account-card" title="Manage accounts">
      <img class="avatar" src="${avatarUrl(active.uuid)}" alt="" />
      <div class="account-text">
        <div class="account-name">${esc(active.username)}</div>
        <div class="account-sub">Microsoft account</div>
      </div>
    </button>`;
  hideBrokenAvatars(box);
  $('#account-card').addEventListener('click', openAccounts);
}

function renderPlay() {
  const inst = selectedInstance();
  const picker = $('#instance-picker');
  picker.innerHTML = state.instances.map((i) => `<option value="${esc(i.id)}">${esc(i.name)}</option>`).join('');
  picker.disabled = state.instances.length === 0;
  if (inst) picker.value = inst.id;

  $('#hero-title').textContent = inst ? inst.name : 'Welcome to MineArcade';
  $('#hero-meta').textContent = inst
    ? `Minecraft ${inst.mcVersion} · ${loaderLabel(inst)} · ${fmtAgo(inst.lastPlayed)}`
    : 'Create your first instance to start playing.';
  $('#instance-tools').hidden = !inst;
  $('.hero-select').hidden = !inst;

  const running = !!inst && state.running.has(inst.id);
  const launching = !!inst && state.launching.has(inst.id);
  const play = $('#play-btn');
  play.classList.toggle('running', running);
  play.disabled = launching;
  play.textContent = !inst ? 'CREATE' : running ? 'STOP' : launching ? 'LOADING…' : 'PLAY';

  const server = state.settings?.featuredServer?.trim();
  const join = $('#join-btn');
  join.hidden = !server || !inst || running;
  join.disabled = launching;
  join.textContent = `Join ${server ?? ''}`;

  renderProgress();
  renderConsole();
}

function renderProgress() {
  const inst = selectedInstance();
  const p = inst && state.progress[inst.id];
  $('#progress').hidden = !p;
  if (!p) return;
  $('#progress-text').textContent = p.message;
  const bar = $('#progress-bar');
  if (p.total > 0) {
    bar.classList.remove('indeterminate');
    bar.style.width = `${Math.round((p.current / p.total) * 100)}%`;
    $('#progress-count').textContent = `${p.current} / ${p.total}`;
  } else {
    bar.classList.add('indeterminate');
    bar.style.width = '';
    $('#progress-count').textContent = '';
  }
}

function logLine(entry) {
  const div = document.createElement('div');
  div.className = `log log-${entry.level.toLowerCase()}`;
  div.textContent = entry.line;
  return div;
}

function renderConsole() {
  const inst = selectedInstance();
  const consoleEl = $('#console');
  const lines = (inst && state.logs[inst.id]) || [];
  if (!lines.length) {
    consoleEl.innerHTML = '<div class="log-empty">Game output will show up here when you launch.</div>';
    return;
  }
  const fragment = document.createDocumentFragment();
  lines.forEach((entry) => fragment.appendChild(logLine(entry)));
  consoleEl.replaceChildren(fragment);
  consoleEl.scrollTop = consoleEl.scrollHeight;
}

function appendLog(instanceId, level, line) {
  const lines = (state.logs[instanceId] ??= []);
  const entry = { level, line };
  lines.push(entry);
  if (lines.length > MAX_LOG_LINES) lines.splice(0, lines.length - MAX_LOG_LINES);

  if (selectedInstance()?.id !== instanceId) return;
  const consoleEl = $('#console');
  const atBottom = consoleEl.scrollHeight - consoleEl.scrollTop - consoleEl.clientHeight < 40;
  consoleEl.querySelector('.log-empty')?.remove();
  consoleEl.appendChild(logLine(entry));
  while (consoleEl.childElementCount > MAX_LOG_LINES) consoleEl.firstElementChild.remove();
  if (atBottom) consoleEl.scrollTop = consoleEl.scrollHeight;
}

function renderInstances() {
  const grid = $('#instance-grid');
  if (!state.instances.length) {
    grid.innerHTML = `
      <div class="empty">
        <img src="assets/logo.png" alt="" />
        <h3>No instances yet</h3>
        <p>An instance is a separate Minecraft install with its own version, mods, worlds and settings.</p>
        <button class="btn btn-primary" data-action="new">Create instance</button>
      </div>`;
    return;
  }
  grid.innerHTML = state.instances
    .map((i) => {
      const running = state.running.has(i.id);
      const launching = state.launching.has(i.id);
      return `
      <article class="card" data-id="${esc(i.id)}" style="--hue:${hueOf(i.id)}">
        <div class="card-banner">
          <span class="card-initial">${esc(i.name.slice(0, 1).toUpperCase())}</span>
          ${running ? '<span class="badge badge-live">Running</span>' : ''}
        </div>
        <div class="card-body">
          <h3 title="${esc(i.name)}">${esc(i.name)}</h3>
          <div class="card-tags">
            <span class="tag">${esc(i.mcVersion)}</span>
            <span class="tag tag-${esc(i.loader)}">${esc(loaderLabel(i))}</span>
          </div>
          <p class="card-meta">${esc(fmtAgo(i.lastPlayed))}${i.autoJoin ? ` · joins ${esc(i.autoJoin)}` : ''}</p>
        </div>
        <div class="card-actions">
          <button class="btn btn-small ${running ? 'btn-danger' : 'btn-primary'}" data-action="play" ${launching ? 'disabled' : ''}>
            ${running ? 'Stop' : launching ? 'Loading…' : 'Play'}
          </button>
          <button class="icon-btn" data-action="edit" title="Edit">${ICONS.edit}</button>
          <button class="icon-btn" data-action="folder" title="Open folder">${ICONS.folder}</button>
          <button class="icon-btn danger" data-action="delete" title="Delete">${ICONS.trash}</button>
        </div>
      </article>`;
    })
    .join('');
}

function renderSettings() {
  const s = state.settings;
  if (!s) return;
  $('#s-client-id').value = s.clientId ?? '';
  $('#s-featured').value = s.featuredServer ?? '';
  $('#s-memory').value = s.defaultMemoryMb;
  $('#s-memory-label').textContent = fmtMem(s.defaultMemoryMb);
  $('#s-hide').checked = !!s.hideOnLaunch;
  if (state.info) {
    $('#s-data-dir').textContent = state.info.dataDir;
    $('#s-data-dir').title = state.info.dataDir;
    $('#s-version').textContent = `MineArcade ${state.info.version}`;
    $('#s-builtin').hidden = !state.info.builtInClientId;
  }
}

// ---------- Actions ----------

async function saveSettings(next) {
  await invoke('save_settings', { settings: next });
  state.settings = next;
}

async function selectInstance(id) {
  try {
    await saveSettings({ ...state.settings, selectedInstance: id });
  } catch (e) {
    toast(String(e), 'error');
  }
  renderPlay();
}

async function reloadInstances() {
  state.instances = await invoke('list_instances');
  render();
}

async function onPlay(server) {
  const inst = selectedInstance();
  if (!inst) return openInstanceEditor();
  if (state.running.has(inst.id)) return stopInstance(inst);
  await launch(inst.id, server);
}

async function stopInstance(inst) {
  const ok = await confirmDialog({
    title: `Stop ${inst.name}?`,
    message: 'This force-closes Minecraft. Unsaved progress in singleplayer worlds may be lost.',
    confirm: 'Stop game',
    danger: true,
  });
  if (ok) await invoke('kill_instance', { instanceId: inst.id });
}

async function launch(instanceId, server = null) {
  if (!state.accounts.active) {
    toast('Sign in with your Microsoft account first.', 'warn');
    startLogin();
    return;
  }
  state.launching.add(instanceId);
  state.logs[instanceId] = [];
  state.progress[instanceId] = { stage: 'start', message: 'Getting ready…', current: 0, total: 0 };
  renderPlay();
  renderInstances();
  try {
    await invoke('launch_instance', { instanceId, server });
  } catch (e) {
    const message = String(e);
    appendLog(instanceId, 'ERROR', message);
    toast(message, 'error', 10000);
  } finally {
    state.launching.delete(instanceId);
    delete state.progress[instanceId];
    renderPlay();
    renderInstances();
  }
}

async function deleteInstance(inst) {
  const ok = await confirmDialog({
    title: `Delete “${inst.name}”?`,
    message: 'This permanently deletes the instance folder, including its worlds, mods, screenshots and options.',
    confirm: 'Delete',
    danger: true,
  });
  if (!ok) return;
  try {
    await invoke('delete_instance', { id: inst.id });
    delete state.logs[inst.id];
    await reloadInstances();
    toast('Instance deleted', 'success');
  } catch (e) {
    toast(String(e), 'error', 8000);
  }
}

// ---------- Instance editor ----------

async function openInstanceEditor(existing = null) {
  const isNew = !existing;
  const draft = existing
    ? { ...existing }
    : { name: '', mcVersion: '', loader: 'vanilla', loaderVersion: null, memoryMb: state.settings.defaultMemoryMb, autoJoin: '', jvmArgs: '', javaPath: '' };

  const body = openModal(`
    <h2>${isNew ? 'New instance' : 'Edit instance'}</h2>
    <label class="field"><span>Name</span>
      <input class="input" id="f-name" maxlength="48" value="${esc(draft.name)}" placeholder="Leave empty to name it after the version" />
    </label>
    <div class="row">
      <label class="field grow"><span>Minecraft version</span>
        <select class="input" id="f-version"><option>Loading versions…</option></select>
      </label>
      <label class="check"><input type="checkbox" id="f-snapshots" /> Snapshots &amp; old versions</label>
    </div>
    <div class="field"><span>Mod loader</span>
      <div class="segmented" id="f-loader">
        <button type="button" data-v="vanilla">Vanilla</button>
        <button type="button" data-v="fabric">Fabric</button>
      </div>
    </div>
    <label class="field" id="f-loader-row"><span>Fabric loader version</span>
      <select class="input" id="f-loader-version"></select>
    </label>
    <label class="field"><span>Memory <b id="f-mem-label"></b></span>
      <input type="range" id="f-memory" min="1024" max="16384" step="512" value="${Number(draft.memoryMb) || 4096}" />
    </label>
    <label class="field"><span>Auto-join server <em>(optional)</em></span>
      <input class="input" id="f-server" value="${esc(draft.autoJoin ?? '')}" placeholder="play.example.org" spellcheck="false" />
    </label>
    <details class="advanced">
      <summary>Advanced</summary>
      <label class="field"><span>Extra JVM arguments</span>
        <input class="input mono" id="f-jvm" value="${esc(draft.jvmArgs ?? '')}" placeholder="-XX:+UseG1GC" spellcheck="false" />
      </label>
      <label class="field"><span>Custom Java executable</span>
        <input class="input mono" id="f-java" value="${esc(draft.javaPath ?? '')}" placeholder="Leave empty to use Mojang's Java (recommended)" spellcheck="false" />
      </label>
    </details>
    <p class="form-error" id="f-error"></p>
    <div class="modal-actions">
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" id="f-save" disabled>${isNew ? 'Create' : 'Save'}</button>
    </div>`);

  const token = modalToken;
  const alive = () => token === modalToken;
  const versionSelect = $('#f-version', body);
  const snapshots = $('#f-snapshots', body);
  const loaderRow = $('#f-loader-row', body);
  const loaderSelect = $('#f-loader-version', body);
  const memory = $('#f-memory', body);
  const errorEl = $('#f-error', body);
  const saveBtn = $('#f-save', body);
  let loader = draft.loader;
  let versionsReady = false;
  let fabricReady = false;

  const updateSave = () => {
    saveBtn.disabled = !versionsReady || (loader === 'fabric' && !fabricReady);
  };
  const updateMemory = () => ($('#f-mem-label', body).textContent = fmtMem(memory.value));
  memory.addEventListener('input', updateMemory);
  updateMemory();

  let fabricRequest = 0;
  async function loadFabric() {
    const mcVersion = versionSelect.value;
    const request = ++fabricRequest;
    fabricReady = false;
    updateSave();
    loaderSelect.disabled = true;
    loaderSelect.innerHTML = '<option>Loading Fabric versions…</option>';
    try {
      const list = await getFabricVersions(mcVersion);
      if (request !== fabricRequest || !alive()) return;
      if (!list.length) {
        loaderSelect.innerHTML = `<option>Fabric doesn't support ${esc(mcVersion)}</option>`;
        return;
      }
      loaderSelect.innerHTML = list
        .map((l) => `<option value="${esc(l.version)}">${esc(l.version)}${l.stable ? '' : ' (beta)'}</option>`)
        .join('');
      const keep = existing?.loader === 'fabric' && existing.mcVersion === mcVersion ? existing.loaderVersion : null;
      const preferred = keep ?? (list.find((l) => l.stable) ?? list[0]).version;
      loaderSelect.value = preferred;
      if (loaderSelect.value !== preferred) loaderSelect.selectedIndex = 0;
      loaderSelect.disabled = false;
      fabricReady = true;
      updateSave();
    } catch (e) {
      if (request === fabricRequest && alive()) loaderSelect.innerHTML = `<option>${esc(String(e))}</option>`;
    }
  }

  const setLoader = (value) => {
    loader = value;
    $$('#f-loader button', body).forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.v === value)));
    loaderRow.hidden = value !== 'fabric';
    if (value === 'fabric' && versionsReady) loadFabric();
    updateSave();
  };
  $$('#f-loader button', body).forEach((b) => b.addEventListener('click', () => setLoader(b.dataset.v)));
  setLoader(loader);

  let manifest;
  try {
    manifest = await getManifest();
  } catch (e) {
    if (alive()) errorEl.textContent = `Could not load the version list: ${e}`;
    return;
  }
  if (!alive()) return;

  const fillVersions = () => {
    const showAll = snapshots.checked;
    const list = manifest.versions.filter((v) => showAll || v.type === 'release');
    const current = versionSelect.value;
    versionSelect.innerHTML = list
      .map((v) => `<option value="${esc(v.id)}">${esc(v.id)}${v.type === 'release' ? '' : ` · ${esc(v.type.replace('_', ' '))}`}</option>`)
      .join('');
    const wanted = [current, draft.mcVersion, manifest.latest.release].find((id) => id && list.some((v) => v.id === id));
    if (wanted) versionSelect.value = wanted;
  };
  const currentType = manifest.versions.find((v) => v.id === draft.mcVersion)?.type;
  snapshots.checked = !!currentType && currentType !== 'release';
  versionSelect.innerHTML = '';
  fillVersions();
  versionsReady = true;
  updateSave();
  if (loader === 'fabric') loadFabric();

  snapshots.addEventListener('change', () => {
    fillVersions();
    if (loader === 'fabric') loadFabric();
  });
  versionSelect.addEventListener('change', () => {
    if (loader === 'fabric') loadFabric();
  });

  saveBtn.addEventListener('click', async () => {
    const mcVersion = versionSelect.value;
    const name = $('#f-name', body).value.trim() || (loader === 'fabric' ? `Fabric ${mcVersion}` : `Minecraft ${mcVersion}`);
    const payload = {
      name,
      mcVersion,
      loader,
      loaderVersion: loader === 'fabric' ? loaderSelect.value : null,
      memoryMb: Number(memory.value),
      autoJoin: $('#f-server', body).value.trim() || null,
      jvmArgs: $('#f-jvm', body).value.trim(),
      javaPath: $('#f-java', body).value.trim() || null,
    };
    saveBtn.disabled = true;
    errorEl.textContent = '';
    try {
      if (isNew) {
        const created = await invoke('create_instance', { draft: payload });
        await saveSettings({ ...state.settings, selectedInstance: created.id });
      } else {
        await invoke('update_instance', { instance: { ...existing, ...payload } });
      }
      closeModal({ silent: true });
      await reloadInstances();
      toast(isNew ? `Created “${name}”` : 'Instance saved', 'success');
    } catch (e) {
      errorEl.textContent = String(e);
      updateSave();
    }
  });
}

// ---------- Accounts ----------

function openAccounts() {
  const rows = state.accounts.accounts
    .map((a) => {
      const active = a.uuid === state.accounts.active;
      return `
      <div class="account-row" data-uuid="${esc(a.uuid)}">
        <img class="avatar" src="${avatarUrl(a.uuid)}" alt="" />
        <div class="grow">
          <div class="account-name">${esc(a.username)}</div>
          <div class="account-sub">${active ? 'Playing as this account' : 'Microsoft account'}</div>
        </div>
        ${active ? '<span class="badge">Active</span>' : '<button class="btn btn-ghost btn-small" data-action="use">Use</button>'}
        <button class="btn btn-ghost btn-small danger" data-action="remove">Sign out</button>
      </div>`;
    })
    .join('');

  const body = openModal(`
    <h2>Accounts</h2>
    <div class="account-list">${rows}</div>
    <div class="modal-actions">
      <button class="btn btn-ghost" data-close>Close</button>
      <button class="btn btn-primary" id="add-account">Add account</button>
    </div>`);

  $('#add-account', body).addEventListener('click', startLogin);
  $('.account-list', body).addEventListener('click', async (event) => {
    const button = event.target.closest('[data-action]');
    if (!button) return;
    const uuid = button.closest('.account-row').dataset.uuid;
    try {
      if (button.dataset.action === 'use') await invoke('set_active_account', { uuid });
      if (button.dataset.action === 'remove') await invoke('remove_account', { uuid });
      state.accounts = await invoke('list_accounts');
      renderAccount();
      if (state.accounts.accounts.length) openAccounts();
      else closeModal();
    } catch (e) {
      toast(String(e), 'error');
    }
  });
}

async function startLogin() {
  let code;
  try {
    code = await invoke('login_start');
  } catch (e) {
    const message = String(e);
    toast(message, 'error', 9000);
    if (message.includes('Settings')) {
      closeModal();
      showPage('settings');
      $('#s-client-id').focus();
    }
    return;
  }

  const body = openModal(
    `<h2>Sign in with Microsoft</h2>
     <p class="muted">Open the Microsoft sign-in page and enter this code:</p>
     <div class="device-code">${esc(code.userCode)}</div>
     <div class="row center"><button class="btn btn-primary" id="copy-open">Copy code &amp; open sign-in page</button></div>
     <p class="muted small center">${esc(code.verificationUri)}</p>
     <div class="waiting" id="login-waiting"><span class="spinner"></span><span id="login-status">Waiting for you to finish signing in…</span></div>
     <div class="modal-actions"><button class="btn btn-ghost" data-close>Cancel</button></div>`,
    { onClose: () => invoke('login_cancel') },
  );

  $('#copy-open', body).addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(code.userCode);
      toast('Code copied to clipboard', 'success', 2500);
    } catch {
      // The code stays visible and selectable in the dialog.
    }
    invoke('open_url', { url: code.verificationUri }).catch((e) => toast(String(e), 'error'));
  });

  try {
    const account = await invoke('login_wait', {
      deviceCode: code.deviceCode,
      interval: code.interval,
      expiresIn: code.expiresIn,
    });
    state.accounts = await invoke('list_accounts');
    if (body.isConnected) closeModal({ silent: true });
    renderAccount();
    toast(`Welcome, ${account.username}!`, 'success');
  } catch (e) {
    const message = String(e);
    if (message === 'cancelled' || !body.isConnected) return;
    $('#login-status', body).textContent = message;
    $('#login-waiting', body).classList.add('error');
  }
}

// ---------- Events ----------

function bindUi() {
  $$('.nav-item').forEach((b) => b.addEventListener('click', () => showPage(b.dataset.page)));

  $('#play-btn').addEventListener('click', () => onPlay(null));
  $('#join-btn').addEventListener('click', () => onPlay(state.settings.featuredServer));
  $('#instance-picker').addEventListener('change', (e) => selectInstance(e.target.value));
  $('#edit-instance-btn').addEventListener('click', () => {
    const inst = selectedInstance();
    if (inst) openInstanceEditor(inst);
  });
  $('#open-folder-btn').addEventListener('click', () => {
    const inst = selectedInstance();
    if (inst) invoke('open_instance_folder', { id: inst.id }).catch((e) => toast(String(e), 'error'));
  });
  $('#clear-log').addEventListener('click', () => {
    const inst = selectedInstance();
    if (inst) state.logs[inst.id] = [];
    renderConsole();
  });

  $('#new-instance-btn').addEventListener('click', () => openInstanceEditor());
  $('#instance-grid').addEventListener('click', async (event) => {
    const button = event.target.closest('[data-action]');
    if (!button) return;
    if (button.dataset.action === 'new') return openInstanceEditor();

    const inst = state.instances.find((i) => i.id === button.closest('.card')?.dataset.id);
    if (!inst) return;
    switch (button.dataset.action) {
      case 'play':
        if (state.running.has(inst.id)) return stopInstance(inst);
        await selectInstance(inst.id);
        showPage('play');
        return launch(inst.id);
      case 'edit':
        return openInstanceEditor(inst);
      case 'folder':
        return invoke('open_instance_folder', { id: inst.id }).catch((e) => toast(String(e), 'error'));
      case 'delete':
        return deleteInstance(inst);
    }
  });

  $('#s-memory').addEventListener('input', (e) => ($('#s-memory-label').textContent = fmtMem(e.target.value)));
  $('#open-data-btn').addEventListener('click', () => invoke('open_data_folder').catch((e) => toast(String(e), 'error')));
  $('#settings-form').addEventListener('submit', async (event) => {
    event.preventDefault();
    try {
      await saveSettings({
        ...state.settings,
        clientId: $('#s-client-id').value.trim(),
        featuredServer: $('#s-featured').value.trim(),
        defaultMemoryMb: Number($('#s-memory').value),
        hideOnLaunch: $('#s-hide').checked,
      });
      renderPlay();
      toast('Settings saved', 'success');
    } catch (e) {
      toast(String(e), 'error');
    }
  });

  // Modal: backdrop click, [data-close] buttons and Escape close it.
  $('#modal').addEventListener('click', (event) => {
    if (event.target.id === 'modal' || event.target.closest('[data-close]')) closeModal();
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !$('#modal').hidden) closeModal();
  });

  // External links open in the default browser.
  document.addEventListener('click', (event) => {
    const link = event.target.closest('[data-url]');
    if (!link) return;
    event.preventDefault();
    invoke('open_url', { url: link.dataset.url }).catch((e) => toast(String(e), 'error'));
  });
}

async function bindBackendEvents() {
  await listen('launch-progress', ({ payload }) => {
    if (!state.launching.has(payload.instanceId)) return;
    state.progress[payload.instanceId] = payload;
    if (selectedInstance()?.id === payload.instanceId) renderProgress();
  });
  await listen('game-log', ({ payload }) => appendLog(payload.instanceId, payload.level, payload.line));
  await listen('game-started', ({ payload }) => {
    state.running.add(payload.instanceId);
    renderPlay();
    renderInstances();
  });
  await listen('game-exit', async ({ payload }) => {
    state.running.delete(payload.instanceId);
    const name = state.instances.find((i) => i.id === payload.instanceId)?.name ?? 'Minecraft';
    if (payload.code !== null && payload.code !== 0) {
      appendLog(payload.instanceId, 'ERROR', `Game exited with code ${payload.code}`);
      toast(`${name} crashed (exit code ${payload.code}). Check the game output for details.`, 'error', 10000);
    } else {
      appendLog(payload.instanceId, 'LAUNCHER', 'Game closed');
    }
    await reloadInstances();
  });
}

async function init() {
  bindUi();
  await bindBackendEvents();
  const [info, settings, instances, accounts, running] = await Promise.all([
    invoke('app_info'),
    invoke('get_settings'),
    invoke('list_instances'),
    invoke('list_accounts'),
    invoke('running_instances'),
  ]);
  Object.assign(state, { info, settings, instances, accounts, running: new Set(running) });
  render();
  renderSettings();
}

init().catch((e) => toast(`MineArcade failed to start: ${e}`, 'error', 15000));
