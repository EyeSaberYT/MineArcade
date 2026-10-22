// Flows shared by several pages: loading data into the store, launching and
// stopping games, saving settings, signing in.
import * as api from './api.js';
import { errorText } from './tauri.js';
import { store, activeAccount } from './store.js';
import { confirmDialog, html, icon, openModal, spinner, toast, toastError } from './ui.js';

// ---------- Loading ----------

export async function refreshInstances() {
  try {
    store.set('instances', await api.instances.list());
  } catch (error) {
    toastError(error, "Couldn't load your instances");
  } finally {
    if (!store.state.instancesLoaded) store.set('instancesLoaded', true);
  }
}

export async function refreshAccounts() {
  try {
    store.set('accounts', await api.auth.accounts());
  } catch (error) {
    toastError(error, "Couldn't load your accounts");
  }
}

export async function refreshRunning() {
  try {
    store.set('running', new Set(await api.launch.running()));
  } catch {
    // The backend may still be starting up.
  }
}

export async function loadSettings() {
  const [settings, info] = await Promise.all([api.system.getSettings(), api.system.appInfo()]);
  store.set('settings', settings);
  store.set('info', info);
  applyTheme(settings.theme);
}

/** Save a partial settings change. Resolves with the saved settings. */
export async function saveSettings(patch) {
  const next = { ...store.state.settings, ...patch };
  const saved = await api.system.saveSettings(next);
  store.set('settings', saved);
  applyTheme(saved.theme);
  return saved;
}

// ---------- Theme ----------

const lightQuery = window.matchMedia('(prefers-color-scheme: light)');

export function applyTheme(theme) {
  const light = theme === 'light' || (theme === 'system' && lightQuery.matches);
  document.documentElement.dataset.theme = light ? 'light' : 'dark';
}

lightQuery.addEventListener('change', () => applyTheme(store.state.settings?.theme ?? 'dark'));

// ---------- Launching ----------

/** The featured server (play.minearcade.org unless changed in Settings). */
export const featuredServer = () => store.state.settings?.featuredServer?.trim() ?? '';

/**
 * Launch an instance, optionally straight into a server or world.
 * quickPlay: { type: 'multiplayer', address } | { type: 'singleplayer', world } | undefined
 */
export async function launchInstance(id, quickPlay) {
  if (store.state.launching.has(id) || store.state.running.has(id)) return;
  if (!activeAccount()) {
    toast('Sign in with your Microsoft account to play.', { kind: 'warn' });
    signIn();
    return;
  }

  store.update('launching', (set) => void set.add(id));
  store.state.console[id] = [];
  store.set('console', store.state.console);

  try {
    const process = await api.launch.instance(id, quickPlay);
    const joining = quickPlay?.type === 'multiplayer' ? quickPlay.address : null;
    toast(joining ? `Joining ${joining}…` : 'Minecraft is starting…', {
      kind: 'success',
      description: `Process ${process.pid}`,
    });
    store.update('running', (set) => void set.add(id));
    if (store.state.settings?.closeOnLaunch) {
      window.__TAURI__?.window?.getCurrentWindow?.().minimize?.();
    }
  } catch (error) {
    toast("Minecraft couldn't start", { kind: 'error', description: errorText(error) });
  } finally {
    store.update('launching', (set) => void set.delete(id));
    refreshRunning();
    refreshInstances();
  }
}

/** Launch straight into the featured MineArcade server. */
export function joinFeaturedServer(id) {
  const address = featuredServer();
  return launchInstance(id, address ? { type: 'multiplayer', address } : undefined);
}

/** The most recently played instance, else the first one, else null. */
export function spotlightInstance() {
  const list = store.state.instances;
  if (!list.length) return null;
  const played = list.filter((i) => i.lastPlayed);
  if (!played.length) return list[0];
  return played.reduce((a, b) => (new Date(a.lastPlayed) > new Date(b.lastPlayed) ? a : b));
}

/**
 * One-click "play on MineArcade": join with the most recently played
 * instance, or set one up first when there are none.
 */
export async function quickJoin() {
  const instance = spotlightInstance();
  if (instance) return joinFeaturedServer(instance.id);
  const { openNewInstanceDialog } = await import('./dialogs/new-instance.js');
  openNewInstanceDialog({
    name: 'MineArcade',
    onCreated: (created) => joinFeaturedServer(created.id),
  });
}

/** Ask, then force-close a running game. */
export async function stopInstance(id, name = 'Minecraft') {
  const ok = await confirmDialog({
    title: `Stop ${name}?`,
    message: 'This force-closes the game. Unsaved progress in singleplayer worlds may be lost.',
    confirmLabel: 'Stop game',
    danger: true,
  });
  if (!ok) return;
  try {
    await api.launch.stop(id);
  } catch (error) {
    // The launcher lost track of it (e.g. after a restart): just clear the flag.
    await api.launch.markStopped(id).catch(() => {});
    store.update('running', (set) => void set.delete(id));
    toastError(error);
  }
}

// ---------- Accounts ----------

/**
 * Microsoft device-code sign-in. The backend emits `auth://device-code`
 * (handled in main.js, which stores it as `deviceCode`) and resolves once the
 * player has approved the sign-in in their browser.
 */
export function signIn() {
  if (store.state.signingIn) {
    showSignInDialog();
    return;
  }
  store.set('deviceCode', null);
  store.set('signingIn', true);
  showSignInDialog();

  api.auth
    .loginMicrosoft()
    .then(async (account) => {
      await refreshAccounts();
      toast(`Signed in as ${account.username}`, { kind: 'success' });
      signInModal?.close();
    })
    .catch((error) => {
      if (signInModal && !signInModal.closed) {
        const status = signInModal.body.querySelector('[data-status]');
        if (status) {
          status.classList.add('danger-text');
          status.textContent = errorText(error);
        }
      } else {
        toast('Sign-in failed', { kind: 'error', description: errorText(error) });
      }
    })
    .finally(() => {
      store.set('signingIn', false);
      store.set('deviceCode', null);
    });
}

let signInModal = null;

function showSignInDialog() {
  if (signInModal && !signInModal.closed) return;

  signInModal = openModal({
    title: 'Sign in with Microsoft',
    description: 'Use the Microsoft account that owns Minecraft: Java Edition. MineArcade never sees your password.',
    size: 'sm',
    body: html`<div data-signin></div>`,
    footer: html`<button class="btn btn-ghost" data-close>Close</button>`,
    onClose: () => {
      off();
      signInModal = null;
    },
  });
  const slot = signInModal.body.querySelector('[data-signin]');

  const render = () => {
    const code = store.state.deviceCode;
    if (!code) {
      slot.innerHTML = String(html`<div class="loading-block">${spinner()} Contacting Microsoft…</div>`);
      return;
    }
    let host = code.verificationUri;
    try {
      host = new URL(code.verificationUri).host;
    } catch {
      // Keep the raw string.
    }
    slot.innerHTML = String(html`
      <div class="stack">
        <p class="muted">Open the Microsoft page and enter this code:</p>
        <div class="device-code selectable">${code.userCode}</div>
        <div class="row">
          <button class="btn btn-primary grow" data-open>${icon('external')} Copy code &amp; open ${host}</button>
        </div>
        <p class="row small muted" data-status style="justify-content:center">${spinner('icon-sm')} Waiting for you to finish signing in…</p>
      </div>`);
    slot.querySelector('[data-open]').addEventListener('click', async () => {
      try {
        await navigator.clipboard.writeText(code.userCode);
        toast('Code copied', { kind: 'success', duration: 2000 });
      } catch {
        // The code stays visible and selectable.
      }
      api.system.openUrl(code.verificationUri).catch(toastError);
    });
  };

  const off = store.on('deviceCode', render);
  render();
}

export async function signOut(account) {
  try {
    store.set('accounts', await api.auth.logout(account.uuid));
    toast(`Signed out ${account.username}`, { kind: 'success' });
  } catch (error) {
    toastError(error);
  }
}

export async function switchAccount(uuid) {
  try {
    store.set('accounts', await api.auth.setActive(uuid));
  } catch (error) {
    toastError(error);
  }
}
