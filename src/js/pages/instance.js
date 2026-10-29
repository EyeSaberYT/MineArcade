// Instance detail: header with play controls, and the content, worlds,
// screenshots, servers, console and settings tabs.
//
// Route: #/instance/:id/:tab?  (tab defaults to `content`)
import * as api from '../api.js';
import { featuredServer, joinFeaturedServer, launchInstance, refreshInstances, stopInstance } from '../actions.js';
import { openScanDialog } from '../dialogs/scan.js';
import { navigate } from '../router.js';
import { store } from '../store.js';
import { errorText, pickSavePath } from '../tauri.js';
import {
  errorBlock,
  formatDuration,
  html,
  icon,
  instanceIcon,
  loaderBadge,
  openModal,
  promptDialog,
  relativeTime,
  setHtml,
  showMenu,
  spinner,
  toast,
  toastError,
} from '../ui.js';
import { mountConsole } from './instance/console.js';
import { mountContent } from './instance/content.js';
import { mountScreenshots } from './instance/screenshots.js';
import { mountServers } from './instance/servers.js';
import { openInstanceSettings } from './instance/settings.js';
import { isLaunching, isRunning, revealInstanceFolder } from './instance/shared.js';
import { mountWorlds } from './instance/worlds.js';

const TABS = [
  { id: 'content', label: 'Content', icon: 'package', mount: mountContent },
  { id: 'worlds', label: 'Worlds', icon: 'globe', mount: mountWorlds },
  { id: 'screenshots', label: 'Screenshots', icon: 'image', mount: mountScreenshots },
  { id: 'servers', label: 'Servers', icon: 'server', mount: mountServers },
  { id: 'console', label: 'Console', icon: 'terminal', mount: mountConsole },
  // As in the Modrinth App, Settings opens a modal over the current tab rather than replacing it.
  { id: 'settings', label: 'Settings', icon: 'settings', modal: true },
];

export function render(root, params) {
  const id = params.id;
  // #/instance/:id/settings shows the Content tab with the settings modal on top.
  const openSettingsOnLoad = params.tab === 'settings';
  let tab = TABS.some((t) => t.id === params.tab && !t.modal) ? params.tab : 'content';
  let instance = null;
  let disposed = false;
  let tabCleanup = null;
  let settingsModal = null;
  const offs = [];

  const ctx = {
    id,
    get instance() {
      return instance;
    },
    /** Replace the loaded instance (after a save) and redraw the header. */
    setInstance(next) {
      instance = next;
      drawHead();
    },
    alive: () => !disposed,
    duplicate: () => duplicate(),
    confirmDelete: () => confirmDelete(),
  };

  // ---------- Header ----------

  function playControls() {
    if (isRunning(id)) {
      return html`<button class="btn btn-danger" data-act="stop" title="Force-close the game">${icon('square')} Stop</button>`;
    }
    if (isLaunching(id)) {
      return html`<button class="btn btn-play" disabled>${spinner()} Starting…</button>`;
    }
    return html`<button class="btn btn-play" data-act="play">${icon('play')} Play</button>`;
  }

  function drawHead() {
    const head = root.querySelector('[data-head]');
    if (!head || !instance) return;
    // Play time and last played change after every session; the summary list is
    // refreshed when the game exits, so prefer its numbers.
    const summary = store.state.instances.find((i) => i.id === id);
    const lastPlayed = summary?.lastPlayed ?? instance.lastPlayed;
    const playTime = summary?.playTimeSecs ?? instance.playTimeSecs;
    const running = isRunning(id);
    const server = featuredServer();

    setHtml(
      head,
      html`
        <div class="inst-hero">
          ${instanceIcon(instance, 76)}
          <div class="grow">
            <div class="row" style="gap:10px">
              <h1 class="truncate">${instance.name}</h1>
              ${running && html`<span class="badge badge-live">Running</span>`}
            </div>
            <div class="row-wrap" style="gap:6px;margin-top:9px">
              <span class="badge badge-mono">${instance.gameVersion}</span>
              ${loaderBadge(instance.loader)}
              ${instance.loader !== 'vanilla' && instance.loaderVersion && html`<span class="badge badge-mono">${instance.loaderVersion}</span>`}
              <span class="small muted inst-hero-meta">
                ${icon('clock', 'icon-sm')} ${playTime ? `${formatDuration(playTime)} played` : 'Not played yet'}
                ${lastPlayed && html` · last played ${relativeTime(lastPlayed)}`}
              </span>
            </div>
          </div>
          <div class="row-wrap inst-hero-actions">
            ${playControls()}
            ${server && !running && html`<button class="btn btn-cyan" data-act="join" ${isLaunching(id) ? 'disabled' : ''} title="Launch straight into ${server}">${icon('server')} Join MineArcade</button>`}
            <button class="btn btn-icon" data-act="folder" title="Open instance folder">${icon('folder')}</button>
            <button class="btn btn-icon" data-act="more" title="More">${icon('more')}</button>
          </div>
        </div>`,
    );
  }

  // ---------- Tabs ----------

  function drawTabs() {
    setHtml(
      root.querySelector('[data-tabs]'),
      TABS.map((t) => {
        // While the modal is open, Settings is the highlighted tab.
        const active = settingsModal ? t.id === 'settings' : t.id === tab;
        return html`<button class="tab ${active ? 'active' : ''}" data-tab="${t.id}">${icon(t.icon, 'icon-sm')} ${t.label}</button>`;
      }),
    );
  }

  function openSettings() {
    if (!instance || settingsModal) return;
    settingsModal = openInstanceSettings(ctx, {
      onClose: () => {
        settingsModal = null;
        if (!disposed) drawTabs();
      },
    });
    drawTabs();
  }

  function mountTab() {
    try {
      tabCleanup?.();
    } catch (error) {
      console.error('tab cleanup failed', error);
    }
    tabCleanup = null;
    const pane = root.querySelector('[data-pane]');
    // A fresh node per tab so listeners from the previous tab can't linger.
    const fresh = document.createElement('div');
    fresh.className = 'inst-pane';
    pane.replaceChildren(fresh);
    const entry = TABS.find((t) => t.id === tab);
    const cleanup = entry.mount(fresh, ctx);
    tabCleanup = typeof cleanup === 'function' ? cleanup : null;
  }

  function switchTab(next) {
    if (TABS.find((t) => t.id === next)?.modal) return openSettings();
    if (next === tab) return;
    tab = next;
    // Keep the URL in step without re-running the router (which would reload the page).
    history.replaceState(null, '', `#/instance/${encodeURIComponent(id)}/${tab}`);
    drawTabs();
    mountTab();
  }

  // ---------- Actions ----------

  async function duplicate() {
    const toastId = toast('Duplicating instance…', { kind: 'loading' });
    try {
      const copy = await api.instances.duplicate(id);
      await refreshInstances();
      toast(`Duplicated as “${copy.name}”`, { kind: 'success', id: toastId });
      if (!disposed) navigate(`/instance/${copy.id}`);
    } catch (error) {
      toast("Couldn't duplicate the instance", { kind: 'error', id: toastId, description: errorText(error) });
    }
  }

  async function exportPack() {
    const target = await pickSavePath({
      title: 'Export instance as .mrpack',
      defaultPath: `${instance.name}.mrpack`,
      extensions: ['mrpack'],
    });
    if (!target) return;
    const toastId = toast('Exporting modpack…', { kind: 'loading' });
    try {
      await api.modpacks.export(id, target);
      toast('Modpack exported', { kind: 'success', id: toastId, description: target });
    } catch (error) {
      toast("Couldn't export the modpack", { kind: 'error', id: toastId, description: errorText(error) });
    }
  }

  async function rename() {
    const name = await promptDialog({ title: 'Rename instance', label: 'Name', value: instance.name });
    if (!name || name === instance.name) return;
    try {
      const updated = await api.instances.rename(id, name);
      refreshInstances();
      if (!disposed) ctx.setInstance(updated);
      toast('Renamed', { kind: 'success' });
    } catch (error) {
      toastError(error, "Couldn't rename the instance");
    }
  }

  function confirmDelete() {
    if (isRunning(id)) {
      toast('Stop the game before deleting this instance.', { kind: 'warn' });
      return;
    }
    const modal = openModal({
      title: `Delete “${instance.name}”?`,
      size: 'sm',
      body: html`
        <p class="muted" style="line-height:1.5">This removes the instance from MineArcade.</p>
        <label class="switch-row">
          <input type="checkbox" data-files checked>
          <span class="switch"></span>
          <span>Also delete its files
            <span class="hint" style="display:block">Worlds, mods, screenshots and settings in the instance folder are removed for good. This can't be undone.</span>
          </span>
        </label>
        <p class="form-error" data-error></p>`,
      footer: html`
        <button class="btn btn-ghost" data-close>Cancel</button>
        <button class="btn btn-danger" data-confirm>${icon('trash')} Delete</button>`,
    });
    const confirm = modal.foot.querySelector('[data-confirm]');
    confirm.addEventListener('click', async () => {
      const deleteFiles = modal.body.querySelector('[data-files]').checked;
      confirm.disabled = true;
      modal.setDismissable(false);
      try {
        await api.instances.remove(id, deleteFiles);
        await refreshInstances();
        modal.setDismissable(true);
        modal.close();
        toast('Instance deleted', { kind: 'success' });
        if (!disposed) navigate('/library');
      } catch (error) {
        modal.setDismissable(true);
        confirm.disabled = false;
        modal.body.querySelector('[data-error]').textContent = errorText(error);
      }
    });
  }

  function openMore(anchor) {
    showMenu(
      anchor,
      [
        { label: 'Scan mods and packs', icon: 'shield', onSelect: () => openScanDialog(id, instance.name) },
        { label: 'Rename', icon: 'pencil', onSelect: rename },
        { label: 'Duplicate', icon: 'copy', onSelect: duplicate },
        { label: 'Export modpack (.mrpack)', icon: 'upload', onSelect: exportPack },
        { separator: true },
        { label: 'Delete instance', icon: 'trash', danger: true, onSelect: confirmDelete },
      ],
      { align: 'end' },
    );
  }

  root.addEventListener('click', (event) => {
    const tabBtn = event.target.closest('[data-tab]');
    if (tabBtn && root.querySelector('[data-tabs]')?.contains(tabBtn)) return switchTab(tabBtn.dataset.tab);
    if (event.target.closest('[data-retry-load]')) return load();

    const act = event.target.closest('[data-act]');
    if (!act || !root.querySelector('[data-head]')?.contains(act) || !instance) return;
    switch (act.dataset.act) {
      case 'play':
        return launchInstance(id);
      case 'stop':
        return stopInstance(id, instance.name);
      case 'join':
        return joinFeaturedServer(id);
      case 'folder':
        return revealInstanceFolder(id);
      case 'more':
        return openMore(act);
    }
  });

  // ---------- Loading ----------

  function skeleton() {
    setHtml(
      root,
      html`
        <div class="skeleton" style="height:118px"></div>
        <div class="skeleton" style="height:40px"></div>
        <div class="skeleton" style="height:300px"></div>`,
    );
  }

  async function load() {
    skeleton();
    try {
      const loaded = await api.instances.get(id);
      if (disposed) return;
      instance = loaded;
    } catch (error) {
      if (disposed) return;
      setHtml(
        root,
        html`
          <a class="back-link" href="#/library">${icon('arrow-left', 'icon-sm')} Library</a>
          ${errorBlock(error, 'retry-load-btn')}`,
      );
      root.querySelector('#retry-load-btn')?.setAttribute('data-retry-load', '');
      return;
    }

    setHtml(
      root,
      html`
        <section class="card inst-head" data-head></section>
        <nav class="tabs" data-tabs></nav>
        <div data-pane></div>`,
    );
    drawHead();
    drawTabs();
    mountTab();
    if (openSettingsOnLoad) {
      history.replaceState(null, '', `#/instance/${encodeURIComponent(id)}/${tab}`);
      openSettings();
    }
  }

  offs.push(store.on(['running', 'launching', 'settings', 'instances'], drawHead));
  load();

  return () => {
    disposed = true;
    // Leaving the page (e.g. the rail or back button) closes the modal; it saves on close.
    settingsModal?.close();
    offs.forEach((off) => off());
    try {
      tabCleanup?.();
    } catch (error) {
      console.error('tab cleanup failed', error);
    }
  };
}
