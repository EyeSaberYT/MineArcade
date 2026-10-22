// Self-updater: signed releases from GitHub, via Tauri's updater plugin.
// State lives in store.state.updater so the startup prompt and the Settings
// page stay in sync:
//   { status: 'idle'|'checking'|'up-to-date'|'available'|'downloading'|'installed'|'error',
//     update, discoveredBy: 'startup'|'manual'|null, progress: { downloaded, total }, error }
import { errorText, updaterPlugin } from './tauri.js';
import { store } from './store.js';
import { formatBytes, toast } from './ui.js';

const STARTUP_DELAY_MS = 3000;
const TOAST_ID = 'minearcade-update';

function setUpdater(patch) {
  store.set('updater', { ...store.state.updater, ...patch });
}

/** Look for a newer version. `silent` swallows errors (used at startup). */
export async function checkForUpdates({ silent = false } = {}) {
  if (!updaterPlugin.available) {
    if (!silent) setUpdater({ status: 'error', error: 'Updates are only available in installed builds.' });
    return;
  }
  setUpdater({ status: 'checking', error: null });
  try {
    const update = await updaterPlugin.check();
    if (update) {
      setUpdater({ status: 'available', update, discoveredBy: silent ? 'startup' : 'manual' });
    } else {
      setUpdater({ status: 'up-to-date', update: null, discoveredBy: null });
    }
  } catch (error) {
    if (silent) setUpdater({ status: 'idle', update: null, discoveredBy: null });
    else setUpdater({ status: 'error', error: errorText(error) });
  }
}

/** Download and install the pending update, with progress in a toast. */
export async function installUpdate() {
  const { update } = store.state.updater;
  if (!update) return;
  setUpdater({ status: 'downloading', progress: { downloaded: 0, total: null }, error: null });

  let total = null;
  let downloaded = 0;
  let lastShown = -1;
  const show = () => {
    const percent = total ? Math.floor((downloaded / total) * 100) : null;
    if (percent !== null && percent === lastShown) return;
    lastShown = percent ?? lastShown;
    toast(`Downloading MineArcade ${update.version}…`, {
      id: TOAST_ID,
      kind: 'loading',
      description: total ? `${formatBytes(downloaded)} / ${formatBytes(total)}` : `${formatBytes(downloaded)} downloaded`,
    });
  };
  show();

  try {
    await update.downloadAndInstall((event) => {
      if (event.event === 'Started') {
        total = event.data.contentLength ?? null;
        downloaded = 0;
      } else if (event.event === 'Progress') {
        downloaded += event.data.chunkLength;
      }
      setUpdater({ progress: { downloaded, total } });
      show();
    });
    setUpdater({ status: 'installed', progress: null });
    toast(`MineArcade ${update.version} installed`, {
      id: TOAST_ID,
      kind: 'success',
      description: 'Restart MineArcade to finish the update.',
      actions: [{ label: 'Restart now', onClick: relaunchApp }],
    });
  } catch (error) {
    setUpdater({ status: 'error', error: errorText(error), progress: null });
    toast("The update couldn't be installed", { id: TOAST_ID, kind: 'error', description: errorText(error) });
  }
}

export function relaunchApp() {
  return updaterPlugin.relaunch();
}

/** Silent check shortly after startup; offers a one-click install when something is found. */
export function startUpdateCheck() {
  if (store.state.settings && store.state.settings.checkUpdatesOnStart === false) return;
  setTimeout(async () => {
    await checkForUpdates({ silent: true });
    const { status, update, discoveredBy } = store.state.updater;
    if (status === 'available' && discoveredBy === 'startup' && update) {
      toast(`MineArcade ${update.version} is available`, {
        id: TOAST_ID,
        description: `You're running ${update.currentVersion}.`,
        actions: [
          { label: 'Update', onClick: installUpdate },
          { label: 'Later', primary: false },
        ],
      });
    }
  }, STARTUP_DELAY_MS);
}
