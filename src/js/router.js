// Hash router. Routes map a pattern to a page module exposing
//   export function render(root, params, query) -> optional cleanup()
// where `root` is an empty <div class="page"> inside the scrolling view.
//
// Routes:
//   #/                                   home
//   #/library                            instance library
//   #/instance/:id/:tab?                 instance detail (tab: content|worlds|screenshots|servers|console|settings)
//   #/discover/:type?                    browse Modrinth / CurseForge (?q=&source=&instance=)
//   #/project/:source/:id                project page (source: modrinth | curseforge; ?instance=)
//   #/settings/:section?                 settings

const routes = [];
let current = { cleanup: null, token: 0 };

export function route(pattern, load) {
  const names = [];
  const regex = new RegExp(
    '^' +
      pattern.replace(/\/:(\w+)(\?)?/g, (_, name, optional) => {
        names.push(name);
        return optional ? '(?:/([^/]+))?' : '/([^/]+)';
      }) +
      '/?$',
  );
  routes.push({ regex, names, load });
}

/** Navigate to a hash path such as `/instance/abc/content`. */
export function navigate(path, { replace = false } = {}) {
  const hash = `#${path.startsWith('/') ? path : `/${path}`}`;
  if (replace) {
    history.replaceState(null, '', hash);
    resolve();
  } else if (location.hash === hash) {
    resolve();
  } else {
    location.hash = hash;
  }
}

/** The current path and query, e.g. { path: '/discover/mod', query: URLSearchParams }. */
export function currentRoute() {
  const raw = location.hash.replace(/^#/, '') || '/';
  const [path, search = ''] = raw.split('?');
  return { path: path || '/', query: new URLSearchParams(search) };
}

const listeners = new Set();
/** Called with the path after each navigation (used by the sidebar). */
export function onNavigate(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

async function resolve() {
  const { path, query } = currentRoute();
  const token = ++current.token;

  let match = null;
  let params = {};
  for (const candidate of routes) {
    const result = candidate.regex.exec(path);
    if (result) {
      match = candidate;
      candidate.names.forEach((name, index) => {
        const value = result[index + 1];
        if (value !== undefined) params[name] = decodeURIComponent(value);
      });
      break;
    }
  }

  try {
    current.cleanup?.();
  } catch (error) {
    console.error('page cleanup failed', error);
  }
  current.cleanup = null;

  listeners.forEach((fn) => fn(path));

  const view = document.getElementById('view');
  const root = document.createElement('div');
  root.className = 'page';
  view.replaceChildren(root);
  view.scrollTop = 0;

  if (!match) {
    root.innerHTML = '<div class="empty"><h3>Page not found</h3><p><a href="#/">Go home</a></p></div>';
    return;
  }

  try {
    const page = await match.load();
    if (token !== current.token) return;
    const cleanup = await page.render(root, params, query);
    if (token !== current.token) {
      cleanup?.();
      return;
    }
    current.cleanup = typeof cleanup === 'function' ? cleanup : null;
  } catch (error) {
    console.error(error);
    if (token === current.token) {
      root.innerHTML = `<div class="empty"><h3>This page failed to load</h3><p class="selectable">${String(error?.message ?? error)
        .replace(/</g, '&lt;')}</p></div>`;
    }
  }
}

export function startRouter() {
  window.addEventListener('hashchange', resolve);
  resolve();
}
