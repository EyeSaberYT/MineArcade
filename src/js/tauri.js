// Thin access layer over Tauri's global API (enabled with `withGlobalTauri`).
// Everything else imports from here rather than touching `window.__TAURI__`.

const T = window.__TAURI__ ?? {};

export const isTauri = Boolean(window.__TAURI__);

/** Invoke a backend command. Rejections are plain strings from the Rust side. */
export function invoke(command, args) {
  if (!T.core) {
    return Promise.reject(new Error(`The MineArcade backend isn't running (${command}).`));
  }
  return T.core.invoke(command, args);
}

/** Subscribe to a backend event; resolves to an unsubscribe function. */
export async function listen(event, handler) {
  if (!T.event) return () => {};
  return T.event.listen(event, (message) => handler(message.payload));
}

/** URL for a local file, served through Tauri's asset protocol. */
export function fileSrc(path) {
  return T.core?.convertFileSrc ? T.core.convertFileSrc(path) : path;
}

/** Normalise an error from `invoke` (or anywhere else) into a message. */
export function errorText(error) {
  if (error == null) return 'Something went wrong.';
  if (typeof error === 'string') return error;
  if (error.message) return error.message;
  return String(error);
}

// ---------- Native dialogs ----------

/** Pick one file. Returns the path, or null when cancelled. */
export async function pickFile({ title, extensions } = {}) {
  if (!T.dialog) return null;
  const result = await T.dialog.open({
    title,
    multiple: false,
    directory: false,
    filters: extensions ? [{ name: 'Supported files', extensions }] : undefined,
  });
  return typeof result === 'string' ? result : null;
}

/** Pick several files. Returns an array (empty when cancelled). */
export async function pickFiles({ title, extensions } = {}) {
  if (!T.dialog) return [];
  const result = await T.dialog.open({
    title,
    multiple: true,
    directory: false,
    filters: extensions ? [{ name: 'Supported files', extensions }] : undefined,
  });
  if (!result) return [];
  return Array.isArray(result) ? result : [result];
}

/** Pick a folder. */
export async function pickDirectory(title) {
  if (!T.dialog) return null;
  const result = await T.dialog.open({ title, directory: true, multiple: false });
  return typeof result === 'string' ? result : null;
}

/** Save-as picker. */
export async function pickSavePath({ title, defaultPath, extensions } = {}) {
  if (!T.dialog) return null;
  const result = await T.dialog.save({
    title,
    defaultPath,
    filters: extensions ? [{ name: 'Supported files', extensions }] : undefined,
  });
  return result ?? null;
}

// ---------- Window drag & drop ----------

/**
 * Listen for files dropped onto the window. `handler` receives
 * `{ type: 'enter' | 'over' | 'drop' | 'leave', paths }`.
 */
export async function onFileDrop(handler) {
  const webview = T.webviewWindow?.getCurrentWebviewWindow?.() ?? T.webview?.getCurrentWebview?.();
  if (!webview?.onDragDropEvent) return () => {};
  return webview.onDragDropEvent((event) => {
    const { type, paths = [] } = event.payload ?? {};
    handler({ type, paths });
  });
}

// ---------- Updater ----------

export const updaterPlugin = {
  available: Boolean(T.updater?.check),
  check: () => T.updater.check(),
  relaunch: () => T.process.relaunch(),
};
