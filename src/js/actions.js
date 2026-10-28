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

// ---------- Accent colour ----------
// Like the theme in Aether, the accent is a per-device preference kept in localStorage.

// The first accent is the stylesheet's own (Modrinth green), so choosing it
// clears the overrides and the light theme's darker green applies again.
export const ACCENTS = [
  { id: 'green', label: 'Modrinth green', color: '#1bd96a' },
  { id: 'pink', label: 'MineArcade pink', color: '#ff3df2' },
  { id: 'red', label: 'Red', color: '#ef5350' },
  { id: 'purple', label: 'Purple', color: '#a78bfa' },
  { id: 'blue', label: 'Blue', color: '#50aaff' },
];

const DEFAULT_ACCENT = ACCENTS[0].id;
// A new key: the old one defaulted to pink, and the look now starts green.
const ACCENT_KEY = 'minearcade.accentChoice';
const ACCENT_PROPERTIES = ['--primary', '--primary-2', '--primary-soft', '--primary-line', '--primary-glow', '--on-primary'];

/** { id, custom } — the chosen accent and the last custom colour. */
export function currentAccent() {
  let id = DEFAULT_ACCENT;
  let custom = '#1bd96a';
  try {
    id = localStorage.getItem(ACCENT_KEY) ?? id;
    custom = localStorage.getItem('minearcade.customAccent') ?? custom;
  } catch {
    // Storage unavailable: defaults.
  }
  if (id !== 'custom' && !ACCENTS.some((a) => a.id === id)) id = DEFAULT_ACCENT;
  return { id, custom };
}

/** Apply (and remember) an accent. `custom` is the colour for the 'custom' accent. */
export function applyAccent(id, custom) {
  try {
    localStorage.setItem(ACCENT_KEY, id);
    if (custom) localStorage.setItem('minearcade.customAccent', custom);
  } catch {
    // Keep it for this session only.
  }
  const color = id === 'custom' ? (custom ?? currentAccent().custom) : ACCENTS.find((a) => a.id === id)?.color;
  const style = document.documentElement.style;
  if (!color || id === DEFAULT_ACCENT) {
    ACCENT_PROPERTIES.forEach((name) => style.removeProperty(name));
    return;
  }
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
  const darker = (c) => Math.round(c * 0.62);
  // Black text on light accents (as Modrinth does on its green), white on dark ones.
  const luminance = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  style.setProperty('--primary', color);
  style.setProperty('--primary-2', `rgb(${darker(r)}, ${darker(g)}, ${darker(b)})`);
  style.setProperty('--primary-soft', `rgba(${r}, ${g}, ${b}, 0.16)`);
  style.setProperty('--primary-line', `rgba(${r}, ${g}, ${b}, 0.45)`);
  style.setProperty('--primary-glow', `rgba(${r}, ${g}, ${b}, 0.25)`);
  style.setProperty('--on-primary', luminance > 0.55 ? '#000000' : '#ffffff');
}

applyAccent(currentAccent().id);

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
    const settings = store.state.settings;
    if (settings?.closeOnLaunch) {
      // The game is its own process and keeps running after the launcher quits.
      setTimeout(() => window.__TAURI__?.process?.exit?.(0), 1500);
    } else if (settings && !settings.keepLauncherOpen) {
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
