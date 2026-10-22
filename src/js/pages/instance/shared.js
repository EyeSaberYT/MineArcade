// Small helpers shared by the instance page's tabs.
import * as api from '../../api.js';
import { store } from '../../store.js';
import { toastError } from '../../ui.js';

/** `3 mods`, `1 mod`. */
export const plural = (count, word, many = `${word}s`) => `${count} ${count === 1 ? word : many}`;

export const isRunning = (id) => store.state.running.has(id);
export const isLaunching = (id) => store.state.launching.has(id);

/** Open a folder in the OS file manager, complaining if it isn't there. */
export async function reveal(path) {
  if (!path) return;
  try {
    await api.system.reveal(path);
  } catch (error) {
    toastError(error);
  }
}

/** Open one of the folders `instance_paths` knows (game, mods, saves, logs…). */
export async function revealInstanceFolder(id, key = 'game') {
  try {
    const paths = await api.instances.paths(id);
    await api.system.reveal(paths[key] ?? paths.game);
  } catch (error) {
    toastError(error);
  }
}
