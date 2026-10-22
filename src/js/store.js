// A tiny reactive store. Pages read `store.state` and subscribe to the keys
// they render with `store.on(keys, fn)`, which returns an unsubscribe function
// to call from the page's cleanup.

const state = {
  /** Settings (see api.js) or null until loaded. */
  settings: null,
  /** AppInfo or null until loaded. */
  info: null,
  /** PublicAccount[] */
  accounts: [],
  /** InstanceSummary[] */
  instances: [],
  instancesLoaded: false,
  /** Ids of instances with a game process running. */
  running: new Set(),
  /** Ids of instances being prepared (downloads, auth, Java) but not yet started. */
  launching: new Set(),
  /** Latest `download://progress` snapshot, or null. */
  progress: null,
  /** Device-code prompt while a Microsoft sign-in is in progress. */
  deviceCode: null,
  signingIn: false,
  /** Game console lines per instance: { [id]: [{ stream, text }] } */
  console: {},
  /** Self-updater: { status, update, discoveredBy, progress: { downloaded, total }, error } */
  updater: { status: 'idle', update: null, discoveredBy: null, progress: null, error: null },
};

const listeners = new Map();

function notify(key) {
  for (const fn of listeners.get(key) ?? []) {
    try {
      fn(state[key], key);
    } catch (error) {
      console.error(`store listener for "${key}" failed`, error);
    }
  }
}

export const store = {
  state,

  /** Replace a key's value and notify its subscribers. */
  set(key, value) {
    state[key] = value;
    notify(key);
  },

  /** Update a key in place (for Sets/objects) and notify. */
  update(key, mutate) {
    const result = mutate(state[key]);
    if (result !== undefined) state[key] = result;
    notify(key);
  },

  /** Subscribe to one or more keys. Returns an unsubscribe function. */
  on(keys, fn) {
    const list = Array.isArray(keys) ? keys : [keys];
    for (const key of list) {
      if (!listeners.has(key)) listeners.set(key, new Set());
      listeners.get(key).add(fn);
    }
    return () => {
      for (const key of list) listeners.get(key)?.delete(fn);
    };
  },
};

/** The active account, or null. */
export function activeAccount() {
  return state.accounts.find((account) => account.active) ?? null;
}

/** Maximum console lines kept per instance. */
export const CONSOLE_LIMIT = 3000;

export function appendConsole(instanceId, stream, text) {
  const lines = (state.console[instanceId] ??= []);
  lines.push({ stream, text });
  if (lines.length > CONSOLE_LIMIT) lines.splice(0, lines.length - CONSOLE_LIMIT);
  notify('console');
}

/** Persisted UI preferences (view modes, dismissed hints, ...). */
export const prefs = {
  get(key, fallback) {
    try {
      const raw = localStorage.getItem(`minearcade.${key}`);
      return raw === null ? fallback : JSON.parse(raw);
    } catch {
      return fallback;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(`minearcade.${key}`, JSON.stringify(value));
    } catch {
      // Storage full or unavailable: preferences are best effort.
    }
  },
};
