// Settings tab: name, icon, notes, memory, arguments, Java, window, Quick Play,
// environment variables and the mod loader build.
import * as api from '../../api.js';
import { featuredServer, refreshInstances } from '../../actions.js';
import { pickIcon } from '../../dialogs/icon-picker.js';
import { errorText, pickDirectory, pickFile } from '../../tauri.js';
import {
  formatMemory,
  html,
  icon,
  instanceIcon,
  loaderLabel,
  setHtml,
  spinner,
  toast,
  toastError,
} from '../../ui.js';

const QUICK_PLAY = [
  { type: 'none', label: 'Title screen' },
  { type: 'multiplayer', label: 'Join a server' },
  { type: 'singleplayer', label: 'Open a world' },
  { type: 'singleplayerContinue', label: 'Continue last world' },
];

const MEMORY_MIN = 512;
const MEMORY_MAX = 32768;
const MEMORY_STEP = 256;

/** One argument per line; blank lines dropped. */
const splitLines = (value) =>
  value
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);

const clone = (value) => JSON.parse(JSON.stringify(value));

export function mountSettings(pane, ctx) {
  const { id } = ctx;
  let draft = clone(ctx.instance);
  let saving = false;
  let loaderRequest = 0;
  let installing = false;

  function quickPlayFor(type) {
    if (type === 'multiplayer') return { type, address: draft.settings.quickPlay.address ?? '' };
    if (type === 'singleplayer') return { type, world: draft.settings.quickPlay.world ?? '' };
    return { type };
  }

  setHtml(
    pane,
    html`
      <div class="stack">
        <section class="panel stack">
          <h3>General</h3>
          <div class="grid-2">
            <label class="field">
              <span>Name</span>
              <input class="input" data-field="name" maxlength="64">
            </label>
            <label class="field">
              <span>Group <em>(optional)</em></span>
              <input class="input" data-field="group" placeholder="e.g. Survival">
            </label>
          </div>
          <div class="field">
            <span class="field-label">Icon</span>
            <div class="row" data-icon-row></div>
          </div>
          <label class="field">
            <span>Notes <em>(optional)</em></span>
            <textarea class="input" data-field="notes" rows="3" placeholder="Anything worth remembering about this instance"></textarea>
          </label>
        </section>

        <section class="panel stack">
          <h3>Memory &amp; Java</h3>
          <div class="grid-2">
            <label class="field">
              <span>Minimum memory <b data-mem-label="memoryMin"></b></span>
              <input type="range" data-mem="memoryMin" min="${MEMORY_MIN}" max="${MEMORY_MAX}" step="${MEMORY_STEP}">
            </label>
            <label class="field">
              <span>Maximum memory <b data-mem-label="memoryMax"></b></span>
              <input type="range" data-mem="memoryMax" min="${MEMORY_MIN}" max="${MEMORY_MAX}" step="${MEMORY_STEP}">
            </label>
          </div>
          <p class="hint">Most modpacks run well with 4–8 GB. Giving Minecraft more than your computer can spare makes it slower, not faster.</p>
          <div class="field">
            <span class="field-label">Java executable <em>(blank = automatic)</em></span>
            <div class="row">
              <input class="input mono" data-field="javaPath" placeholder="Picked automatically for this Minecraft version" spellcheck="false">
              <button class="btn btn-outline" data-browse-java>${icon('folder', 'icon-sm')} Browse</button>
            </div>
          </div>
          <label class="field">
            <span>JVM arguments <em>(one per line)</em></span>
            <textarea class="input mono" data-lines="jvmArgs" rows="4" placeholder="-XX:+UseG1GC" spellcheck="false"></textarea>
          </label>
          <label class="field">
            <span>Game arguments <em>(one per line)</em></span>
            <textarea class="input mono" data-lines="gameArgs" rows="3" spellcheck="false"></textarea>
          </label>
          <div class="field">
            <span class="field-label">Environment variables</span>
            <div class="stack-sm" data-env></div>
            <div><button class="btn btn-ghost btn-sm" data-env-add>${icon('plus', 'icon-sm')} Add variable</button></div>
          </div>
        </section>

        <section class="panel stack">
          <h3>Window &amp; Quick Play</h3>
          <div class="grid-2">
            <label class="field">
              <span>Width</span>
              <input class="input" type="number" min="320" step="1" data-num="width">
            </label>
            <label class="field">
              <span>Height</span>
              <input class="input" type="number" min="240" step="1" data-num="height">
            </label>
          </div>
          <label class="switch-row">
            <input type="checkbox" data-fullscreen>
            <span class="switch"></span>
            <span>Launch in fullscreen</span>
          </label>
          <div class="field">
            <span class="field-label">When the game starts</span>
            <div class="segmented" data-quickplay></div>
            <div data-quickplay-extra></div>
            <span class="hint">Quick Play needs Minecraft 1.20 or newer; older versions open the title screen.</span>
          </div>
          <div class="field">
            <span class="field-label">Game directory <em>(blank = the instance folder)</em></span>
            <div class="row">
              <input class="input mono" data-field="gameDir" placeholder="Default" spellcheck="false">
              <button class="btn btn-outline" data-browse-dir>${icon('folder', 'icon-sm')} Browse</button>
            </div>
          </div>
        </section>

        <section class="panel stack" data-loader></section>

        <div class="row inst-save-bar">
          <span class="small muted grow" data-save-status></span>
          <button class="btn btn-ghost" data-reset>Discard changes</button>
          <button class="btn btn-primary" data-save>${icon('save', 'icon-sm')} Save changes</button>
        </div>
      </div>`,
  );

  const $ = (selector) => pane.querySelector(selector);
  const saveBtn = $('[data-save]');

  // ---------- Filling the form from the draft ----------

  function drawIcon() {
    setHtml(
      $('[data-icon-row]'),
      html`${instanceIcon({ icon: draft.icon, name: draft.name || 'Instance', id }, 52)}
        <button class="btn btn-outline btn-sm" data-pick-icon>${icon('image', 'icon-sm')} Change icon</button>`,
    );
  }

  function drawMemory() {
    for (const key of ['memoryMin', 'memoryMax']) {
      $(`[data-mem="${key}"]`).value = draft.settings[key];
      $(`[data-mem-label="${key}"]`).textContent = formatMemory(draft.settings[key]);
    }
  }

  function drawEnv() {
    const env = draft.settings.env ?? [];
    setHtml(
      $('[data-env]'),
      env.length
        ? env.map(
            ([key, value], index) => html`
              <div class="row" data-env-index="${index}">
                <input class="input mono" data-env-key value="${key}" placeholder="NAME" spellcheck="false" style="max-width:220px">
                <span class="muted">=</span>
                <input class="input mono" data-env-value value="${value}" placeholder="value" spellcheck="false">
                <button class="btn btn-icon btn-sm danger" data-env-remove title="Remove">${icon('x', 'icon-sm')}</button>
              </div>`,
          )
        : html`<span class="hint">None. These are passed to the game process, e.g. for a GPU or mod setting.</span>`,
    );
  }

  function drawQuickPlay() {
    const qp = draft.settings.quickPlay ?? { type: 'none' };
    setHtml(
      $('[data-quickplay]'),
      QUICK_PLAY.map((o) => html`<button data-qp="${o.type}" aria-pressed="${String(qp.type === o.type)}">${o.label}</button>`),
    );
    const extra = $('[data-quickplay-extra]');
    const server = featuredServer();
    if (qp.type === 'multiplayer') {
      setHtml(
        extra,
        html`<div class="row">
          <input class="input mono" data-qp-address value="${qp.address ?? ''}" placeholder="play.example.com" spellcheck="false">
          ${server && html`<button class="btn btn-cyan btn-sm" data-qp-featured>${icon('server', 'icon-sm')} Use ${server}</button>`}
        </div>`,
      );
    } else if (qp.type === 'singleplayer') {
      setHtml(
        extra,
        html`<input class="input" data-qp-world value="${qp.world ?? ''}" placeholder="World folder name" list="inst-worlds-${id}" spellcheck="false">
          <datalist id="inst-worlds-${id}" data-worlds></datalist>`,
      );
      // Suggest the instance's worlds; typing a name works regardless.
      api.worlds
        .overview(id)
        .then((overview) => {
          const list = ctx.alive() && pane.querySelector('[data-worlds]');
          if (list) setHtml(list, overview.worlds.map((w) => html`<option value="${w.name}"></option>`));
        })
        .catch(() => {});
    } else {
      setHtml(extra, '');
    }
  }

  function fill() {
    $('[data-field="name"]').value = draft.name ?? '';
    $('[data-field="group"]').value = draft.group ?? '';
    $('[data-field="notes"]').value = draft.notes ?? '';
    $('[data-field="javaPath"]').value = draft.settings.javaPath ?? '';
    $('[data-field="gameDir"]').value = draft.settings.gameDir ?? '';
    $('[data-lines="jvmArgs"]').value = (draft.settings.jvmArgs ?? []).join('\n');
    $('[data-lines="gameArgs"]').value = (draft.settings.gameArgs ?? []).join('\n');
    $('[data-num="width"]').value = draft.settings.width;
    $('[data-num="height"]').value = draft.settings.height;
    $('[data-fullscreen]').checked = Boolean(draft.settings.fullscreen);
    drawIcon();
    drawMemory();
    drawEnv();
    drawQuickPlay();
    drawLoader();
    markDirty();
  }

  // ---------- Dirty tracking & validation ----------

  /** The instance as it would be saved. */
  function collect(source = draft) {
    const next = clone(source);
    next.name = next.name.trim();
    next.group = next.group?.trim() || null;
    next.notes = next.notes?.trim() ? next.notes : null;
    next.settings.javaPath = next.settings.javaPath?.trim() || null;
    next.settings.gameDir = next.settings.gameDir?.trim() || null;
    next.settings.env = (next.settings.env ?? [])
      .map(([k, v]) => [k.trim(), v])
      .filter(([k]) => k);
    const qp = next.settings.quickPlay;
    if (qp.type === 'multiplayer') qp.address = (qp.address ?? '').trim();
    if (qp.type === 'singleplayer') qp.world = (qp.world ?? '').trim();
    return next;
  }

  function problem(next) {
    if (!next.name) return 'The name can’t be empty.';
    if (next.settings.memoryMin > next.settings.memoryMax) return 'Minimum memory is above the maximum.';
    if (!(next.settings.width >= 320) || !(next.settings.height >= 240)) return 'The window must be at least 320 × 240.';
    if (next.settings.quickPlay.type === 'multiplayer' && !next.settings.quickPlay.address) return 'Enter a server address for Quick Play.';
    if (next.settings.quickPlay.type === 'singleplayer' && !next.settings.quickPlay.world) return 'Enter a world for Quick Play.';
    if (next.settings.env.some(([k]) => /[\s=]/.test(k))) return 'Variable names can’t contain spaces or “=”.';
    return null;
  }

  function markDirty() {
    const next = collect();
    // Normalise both sides so `""` vs `null` doesn't count as a change.
    const dirty = JSON.stringify(next) !== JSON.stringify(collect(ctx.instance));
    const issue = problem(next);
    const status = $('[data-save-status]');
    status.textContent = issue ?? (dirty ? 'Unsaved changes' : '');
    status.className = `small grow ${issue ? 'danger-text' : dirty ? 'warn-text' : 'muted'}`;
    saveBtn.disabled = saving || !dirty || Boolean(issue);
    $('[data-reset]').disabled = saving || !dirty;
  }

  // ---------- Loader ----------

  async function drawLoader() {
    const section = $('[data-loader]');
    const current = ctx.instance;
    if (current.loader === 'vanilla') {
      setHtml(
        section,
        html`<h3>Mod loader</h3>
          <p class="hint">This is a vanilla ${current.gameVersion} instance, so there's no loader to manage. Create a new instance to play with Fabric, Quilt, Forge or NeoForge.</p>`,
      );
      return;
    }
    const request = ++loaderRequest;
    setHtml(
      section,
      html`<h3>${loaderLabel(current.loader)} loader</h3>
        <p class="hint">Installed build: <code>${current.loaderVersion ?? 'not installed yet'}</code>. Changing it reinstalls the loader for Minecraft ${current.gameVersion}; your mods stay in place.</p>
        <div class="row">
          <select class="input" data-loader-version disabled><option>Loading builds…</option></select>
          <button class="btn btn-outline" data-install-loader disabled>${icon('download', 'icon-sm')} Install</button>
        </div>`,
    );
    try {
      const list = await api.versions.loaderVersions(current.loader, current.gameVersion);
      if (request !== loaderRequest || !ctx.alive()) return;
      const select = section.querySelector('[data-loader-version]');
      if (!list.length) {
        setHtml(select, html`<option value="">No ${loaderLabel(current.loader)} builds for ${current.gameVersion}</option>`);
        return;
      }
      setHtml(
        select,
        list.map(
          (entry) => html`<option value="${entry.version}">${entry.version}${entry.stable ? ' · stable' : ''}${entry.version === current.loaderVersion ? ' · installed' : ''}</option>`,
        ),
      );
      select.value = list.some((e) => e.version === current.loaderVersion)
        ? current.loaderVersion
        : (list.find((e) => e.stable) ?? list[0]).version;
      select.disabled = installing;
      updateInstallButton();
    } catch (error) {
      if (request !== loaderRequest || !ctx.alive()) return;
      setHtml(section.querySelector('[data-loader-version]'), html`<option value="">Couldn't load builds: ${errorText(error)}</option>`);
    }
  }

  function updateInstallButton() {
    const select = pane.querySelector('[data-loader-version]');
    const button = pane.querySelector('[data-install-loader]');
    if (!select || !button) return;
    const same = select.value === ctx.instance.loaderVersion;
    button.disabled = installing || !select.value;
    setHtml(button, installing ? html`${spinner('icon-sm')} Installing…` : html`${icon('download', 'icon-sm')} ${same ? 'Reinstall' : 'Install'}`);
  }

  async function installLoader() {
    const version = pane.querySelector('[data-loader-version]')?.value;
    if (!version || installing) return;
    installing = true;
    updateInstallButton();
    pane.querySelector('[data-loader-version]').disabled = true;
    const label = `${loaderLabel(ctx.instance.loader)} ${version}`;
    try {
      const updated = await api.instances.installLoader(id, version);
      toast(`Installed ${label}`, { kind: 'success' });
      refreshInstances();
      installing = false;
      if (!ctx.alive()) return;
      // Only the loader build changed; keep the rest of the draft.
      draft.loaderVersion = updated.loaderVersion;
      ctx.setInstance({ ...ctx.instance, loaderVersion: updated.loaderVersion });
      drawLoader();
      markDirty();
    } catch (error) {
      installing = false;
      toastError(error, `Couldn't install ${label}`);
      if (!ctx.alive()) return;
      pane.querySelector('[data-loader-version]').disabled = false;
      updateInstallButton();
    }
  }

  // ---------- Saving ----------

  async function save() {
    const next = collect();
    if (problem(next)) return markDirty();
    saving = true;
    saveBtn.disabled = true;
    setHtml(saveBtn, html`${spinner('icon-sm')} Saving…`);
    try {
      const saved = await api.instances.update(next);
      toast('Instance settings saved', { kind: 'success' });
      refreshInstances();
      if (ctx.alive()) {
        ctx.setInstance(saved);
        draft = clone(saved);
      }
    } catch (error) {
      toastError(error, "Couldn't save the settings");
    }
    saving = false;
    if (!ctx.alive()) return;
    setHtml(saveBtn, html`${icon('save', 'icon-sm')} Save changes`);
    fill();
  }

  // ---------- Events ----------

  pane.addEventListener('input', (event) => {
    const t = event.target;
    if (t.dataset.field) {
      const key = t.dataset.field;
      if (key === 'javaPath' || key === 'gameDir') draft.settings[key] = t.value;
      else draft[key] = t.value;
    } else if (t.dataset.lines) {
      draft.settings[t.dataset.lines] = splitLines(t.value);
    } else if (t.dataset.num) {
      draft.settings[t.dataset.num] = Math.round(Number(t.value)) || 0;
    } else if (t.dataset.mem) {
      const key = t.dataset.mem;
      draft.settings[key] = Number(t.value);
      // Keep min ≤ max by dragging the other slider along.
      if (key === 'memoryMin' && draft.settings.memoryMax < draft.settings.memoryMin) draft.settings.memoryMax = draft.settings.memoryMin;
      if (key === 'memoryMax' && draft.settings.memoryMin > draft.settings.memoryMax) draft.settings.memoryMin = draft.settings.memoryMax;
      drawMemory();
    } else if (t.matches('[data-qp-address]')) {
      draft.settings.quickPlay = { type: 'multiplayer', address: t.value };
    } else if (t.matches('[data-qp-world]')) {
      draft.settings.quickPlay = { type: 'singleplayer', world: t.value };
    } else if (t.matches('[data-env-key], [data-env-value]')) {
      const index = Number(t.closest('[data-env-index]').dataset.envIndex);
      draft.settings.env[index][t.matches('[data-env-key]') ? 0 : 1] = t.value;
    } else {
      return;
    }
    if (t.dataset.field === 'name') drawIcon();
    markDirty();
  });

  pane.addEventListener('change', (event) => {
    const t = event.target;
    if (t.matches('[data-fullscreen]')) {
      draft.settings.fullscreen = t.checked;
      markDirty();
    } else if (t.matches('[data-loader-version]')) {
      updateInstallButton();
    }
  });

  pane.addEventListener('click', async (event) => {
    const t = event.target;
    const qp = t.closest('[data-qp]');
    if (qp) {
      draft.settings.quickPlay = quickPlayFor(qp.dataset.qp);
      drawQuickPlay();
      return markDirty();
    }
    if (t.closest('[data-qp-featured]')) {
      draft.settings.quickPlay = { type: 'multiplayer', address: featuredServer() };
      drawQuickPlay();
      return markDirty();
    }
    const iconBtn = t.closest('[data-pick-icon]');
    if (iconBtn) {
      const chosen = await pickIcon(iconBtn, draft.icon);
      if (chosen === undefined || !ctx.alive()) return;
      draft.icon = chosen;
      drawIcon();
      return markDirty();
    }
    if (t.closest('[data-browse-java]')) {
      const path = await pickFile({ title: 'Choose a Java executable (java / javaw)' });
      if (!path || !ctx.alive()) return;
      draft.settings.javaPath = path;
      $('[data-field="javaPath"]').value = path;
      return markDirty();
    }
    if (t.closest('[data-browse-dir]')) {
      const path = await pickDirectory('Choose a game directory');
      if (!path || !ctx.alive()) return;
      draft.settings.gameDir = path;
      $('[data-field="gameDir"]').value = path;
      return markDirty();
    }
    if (t.closest('[data-env-add]')) {
      draft.settings.env = [...(draft.settings.env ?? []), ['', '']];
      drawEnv();
      pane.querySelector('[data-env-index]:last-child [data-env-key]')?.focus();
      return markDirty();
    }
    if (t.closest('[data-env-remove]')) {
      const index = Number(t.closest('[data-env-index]').dataset.envIndex);
      draft.settings.env.splice(index, 1);
      drawEnv();
      return markDirty();
    }
    if (t.closest('[data-install-loader]')) return installLoader();
    if (t.closest('[data-reset]')) {
      draft = clone(ctx.instance);
      return fill();
    }
    if (t.closest('[data-save]')) return save();
  });

  fill();
}
