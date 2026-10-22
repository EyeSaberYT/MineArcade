// Servers tab: the instance's in-game multiplayer list (`servers.dat`).
// Edits are held as a draft and written as a whole on Save. The game rewrites
// the file whenever the list changes in game, so saving waits until it closes.
import * as api from '../../api.js';
import { launchInstance } from '../../actions.js';
import { store } from '../../store.js';
import {
  emptyState,
  errorBlock,
  html,
  icon,
  loadingBlock,
  openModal,
  setHtml,
  showMenu,
  spinner,
  toast,
  toastError,
} from '../../ui.js';
import { isLaunching, isRunning, plural, revealInstanceFolder } from './shared.js';

const FIELDS = ['name', 'address', 'icon', 'hidden', 'acceptTextures'];
const sameList = (a, b) => a.length === b.length && a.every((entry, i) => FIELDS.every((f) => entry[f] === b[i][f]));

/** Mirrors the backend's validation for the address field. */
function addressProblem(address) {
  if (!address) return 'An address is required.';
  if (/\s/.test(address)) return 'Addresses can\'t contain spaces — try "play.example.com:25565".';
  return null;
}

/** Server icons are base64 PNGs without a `data:` prefix, as the game caches them. */
function serverIcon(entry) {
  const fallback = html`<div class="inst-thumb">${icon('server', 'icon-sm')}</div>`;
  if (!entry.icon || !/^[A-Za-z0-9+/=\s]+$/.test(entry.icon)) return fallback;
  return html`<div class="inst-thumb"><img src="data:image/png;base64,${entry.icon.replace(/\s/g, '')}" alt="" data-fallback="${String(fallback)}"></div>`;
}

export function mountServers(pane, ctx) {
  const { id } = ctx;
  let file = null;
  let saved = [];
  let draft = [];
  let loading = true;
  let loadError = null;
  let saving = false;
  let editor = null;

  function draw() {
    if (loading) return setHtml(pane, loadingBlock('Reading the server list…'));
    if (loadError) {
      setHtml(pane, errorBlock(loadError, 'servers-retry'));
      pane.querySelector('#servers-retry')?.setAttribute('data-reload', '');
      return;
    }
    const running = isRunning(id);
    const launching = isLaunching(id);
    const dirty = !sameList(saved, draft);

    setHtml(
      pane,
      html`
        <div class="stack">
          ${running &&
          html`<div class="inst-note inst-note-warn">
            ${icon('alert', 'icon-sm')}
            <div><strong>This instance is running</strong>
              <div>Minecraft rewrites its server list whenever you change it in game, so MineArcade can't save until the game closes. Your edits can stay here until then.</div></div>
          </div>`}

          <div class="row-between">
            <div class="row">
              <button class="btn btn-outline btn-sm" data-add>${icon('plus', 'icon-sm')} Add server</button>
              <span class="small muted">${draft.length ? `${plural(draft.length, 'server')} in the game's list` : 'No servers yet'}</span>
            </div>
            <div class="row" style="gap:8px">
              ${dirty && html`<span class="small warn-text">Unsaved changes</span>`}
              <button class="btn btn-ghost btn-sm" data-revert ${!dirty || saving ? 'disabled' : ''}>${icon('rotate-ccw', 'icon-sm')} Revert</button>
              <button class="btn btn-primary btn-sm" data-save ${!dirty || saving || running ? 'disabled' : ''} title="${running ? 'Close the game before saving' : ''}">
                ${saving ? spinner('icon-sm') : icon('save', 'icon-sm')} Save
              </button>
            </div>
          </div>

          ${draft.length === 0
            ? emptyState({
                iconName: 'server',
                title: 'No multiplayer servers',
                description: "Servers added here turn up in the game's multiplayer screen, and servers you add in game turn up here.",
                actions: html`<button class="btn btn-primary btn-sm" data-add>${icon('plus', 'icon-sm')} Add server</button>`,
              })
            : html`<div class="list">${draft.map(
                (entry, index) => html`
                  <div class="list-row" data-index="${index}">
                    ${serverIcon(entry)}
                    <div class="grow">
                      <div class="row" style="gap:8px">
                        <strong class="truncate">${entry.name || entry.address}</strong>
                        ${entry.hidden && html`<span class="badge">${icon('eye-off', 'icon-sm')} Hidden</span>`}
                      </div>
                      <div class="tiny muted mono truncate">${entry.address || 'No address'}${entry.acceptTextures ? ' · resource packs allowed' : ''}</div>
                    </div>
                    <div class="row-actions">
                      <button class="btn btn-play btn-sm" data-join ${running || launching || !entry.address ? 'disabled' : ''}
                        title="${running ? 'This instance is already running' : `Start Minecraft on ${entry.address}`}">
                        ${launching ? spinner('icon-sm') : icon('play', 'icon-sm')} ${launching ? 'Launching' : 'Join'}
                      </button>
                      <button class="btn btn-ghost btn-icon btn-sm" data-row-menu title="More">${icon('more', 'icon-sm')}</button>
                    </div>
                  </div>`,
              )}</div>`}

          <div class="row-between small muted">
            <span class="truncate" title="${file?.path ?? ''}">${file?.exists ? 'Saved to ' : 'Will be saved to '}<span class="mono">${file?.path ?? ''}</span></span>
            <button class="btn btn-ghost btn-sm" data-folder>${icon('folder', 'icon-sm')} Open folder</button>
          </div>
        </div>`,
    );
  }

  async function load() {
    loading = true;
    draw();
    try {
      const list = await api.servers.list(id);
      if (!ctx.alive()) return;
      file = list;
      saved = list.servers;
      draft = list.servers.map((s) => ({ ...s }));
      loadError = null;
    } catch (error) {
      if (!ctx.alive()) return;
      loadError = error;
    }
    loading = false;
    draw();
  }

  async function save() {
    saving = true;
    draw();
    try {
      const list = await api.servers.save(id, draft);
      toast(`Saved ${plural(list.servers.length, 'server')}`, { kind: 'success' });
      if (!ctx.alive()) return;
      file = list;
      saved = list.servers;
      draft = list.servers.map((s) => ({ ...s }));
    } catch (error) {
      toastError(error, "Couldn't save the server list");
    }
    saving = false;
    if (ctx.alive()) draw();
  }

  function move(index, by) {
    const target = index + by;
    if (target < 0 || target >= draft.length) return;
    const [entry] = draft.splice(index, 1);
    draft.splice(target, 0, entry);
    draw();
  }

  /** Add (index null) or edit one server. */
  function openEditor(index) {
    editor?.close();
    const entry = index == null ? null : draft[index];
    const modal = openModal({
      title: entry ? 'Edit server' : 'Add server',
      description: 'The game’s own fields, kept exactly as the game stores them.',
      size: 'sm',
      body: html`
        ${isRunning(id) &&
        html`<div class="inst-note inst-note-warn">${icon('alert', 'icon-sm')}<div>The game is running: you can edit now, but saving waits until it closes.</div></div>`}
        <label class="field">
          <span>Address</span>
          <input class="input mono" data-address value="${entry?.address ?? ''}" placeholder="play.example.com" spellcheck="false" autofocus>
          <span class="form-error" data-problem></span>
        </label>
        <label class="field">
          <span>Name</span>
          <input class="input" data-name value="${entry?.name ?? ''}" placeholder="My server">
          <span class="hint">Shown in the game's server list. Defaults to the address.</span>
        </label>
        <label class="switch-row">
          <input type="checkbox" data-hidden ${entry?.hidden ? 'checked' : ''}>
          <span class="switch"></span>
          <span>Hidden<span class="hint" style="display:block">Only listed in game while hidden servers are shown.</span></span>
        </label>
        <label class="switch-row">
          <input type="checkbox" data-textures ${entry?.acceptTextures ? 'checked' : ''}>
          <span class="switch"></span>
          <span>Resource packs<span class="hint" style="display:block">Whether this server may send its own packs.</span></span>
        </label>`,
      footer: html`
        <button class="btn btn-ghost" data-close>Cancel</button>
        <button class="btn btn-primary" data-submit>${entry ? 'Apply' : 'Add server'}</button>`,
      onClose: () => {
        if (editor === modal) editor = null;
      },
    });
    editor = modal;

    const addressInput = modal.body.querySelector('[data-address]');
    const problemEl = modal.body.querySelector('[data-problem]');
    const submitBtn = modal.foot.querySelector('[data-submit]');
    const validate = () => {
      const problem = addressProblem(addressInput.value.trim());
      problemEl.textContent = addressInput.value && problem ? problem : '';
      submitBtn.disabled = Boolean(problem);
      return !problem;
    };
    const submit = () => {
      if (!validate()) return;
      const address = addressInput.value.trim();
      const next = {
        // The game shows the address when a server has no name.
        name: modal.body.querySelector('[data-name]').value.trim() || address,
        address,
        // The cached icon is carried through untouched.
        icon: entry?.icon ?? null,
        hidden: modal.body.querySelector('[data-hidden]').checked,
        acceptTextures: modal.body.querySelector('[data-textures]').checked,
      };
      // The row may have moved or gone since the editor opened.
      const at = entry ? draft.indexOf(entry) : -1;
      if (at >= 0) draft[at] = next;
      else draft.push(next);
      modal.close();
      if (ctx.alive()) draw();
    };
    addressInput.addEventListener('input', validate);
    modal.body.addEventListener('keydown', (event) => event.key === 'Enter' && event.target.matches('input.input') && submit());
    submitBtn.addEventListener('click', submit);
    validate();
  }

  pane.addEventListener('click', (event) => {
    const t = event.target;
    if (t.closest('[data-reload]')) return load();
    if (t.closest('[data-add]')) return openEditor(null);
    if (t.closest('[data-folder]')) return revealInstanceFolder(id);
    if (t.closest('[data-save]')) return save();
    if (t.closest('[data-revert]')) {
      draft = saved.map((s) => ({ ...s }));
      return draw();
    }
    const rowEl = t.closest('[data-index]');
    if (!rowEl) return;
    const index = Number(rowEl.dataset.index);
    const entry = draft[index];
    if (!entry) return;
    if (t.closest('[data-join]')) return launchInstance(id, { type: 'multiplayer', address: entry.address });
    const menuBtn = t.closest('[data-row-menu]');
    if (menuBtn) {
      showMenu(
        menuBtn,
        [
          { label: 'Edit', icon: 'pencil', onSelect: () => openEditor(index) },
          { label: 'Move up', icon: 'chevron-up', disabled: index === 0, onSelect: () => move(index, -1) },
          { label: 'Move down', icon: 'chevron-down', disabled: index === draft.length - 1, onSelect: () => move(index, 1) },
          { separator: true },
          {
            label: 'Remove',
            icon: 'trash',
            danger: true,
            onSelect: () => {
              draft.splice(index, 1);
              draw();
            },
          },
        ],
        { align: 'end' },
      );
    }
  });

  // The game may have rewritten servers.dat; pick that up unless there are unsaved edits.
  let wasRunning = isRunning(id);
  const off = store.on(['running', 'launching'], () => {
    const now = isRunning(id);
    if (wasRunning && !now && sameList(saved, draft)) load();
    else if (!loading) draw();
    wasRunning = now;
  });
  load();
  return () => {
    off();
    editor?.close();
  };
}
