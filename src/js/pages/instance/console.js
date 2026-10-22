// Console tab: live game output from `store.state.console[id]`, appended
// incrementally, with follow-scroll, clear, copy and "load last log".
import * as api from '../../api.js';
import { CONSOLE_LIMIT, store } from '../../store.js';
import { html, icon, setHtml, spinner, toast, toastError } from '../../ui.js';
import { isRunning, plural } from './shared.js';

/** Distance from the bottom, in pixels, that still counts as "following". */
const FOLLOW_THRESHOLD = 24;

function lineClass({ stream, text }) {
  if (stream === 'launcher') return 'log-line log-launcher';
  if (/\/(ERROR|FATAL)\]|\bFATAL\b/.test(text) || (stream === 'stderr' && /Exception|^\s+at\s|Caused by:/.test(text))) {
    return 'log-line log-error';
  }
  if (/\/WARN(ING)?\]/.test(text)) return 'log-line log-warn';
  if (/\/DEBUG\]/.test(text)) return 'log-line log-debug';
  return 'log-line';
}

function lineNode(line) {
  const div = document.createElement('div');
  div.className = lineClass(line);
  div.textContent = line.text;
  return div;
}

export function mountConsole(pane, ctx) {
  const { id } = ctx;
  let follow = true;
  let lastRendered = null;
  let loadingLog = false;

  setHtml(
    pane,
    html`
      <div class="stack">
        <div class="row-between">
          <div class="row" data-status></div>
          <div class="row" style="gap:6px">
            <button class="btn btn-ghost btn-sm" data-jump hidden>${icon('chevron-down', 'icon-sm')} Jump to end</button>
            <button class="btn btn-outline btn-sm" data-load-log>${icon('file', 'icon-sm')} Load last log</button>
            <button class="btn btn-outline btn-sm" data-copy>${icon('copy', 'icon-sm')} Copy all</button>
            <button class="btn btn-outline btn-sm" data-clear>${icon('x', 'icon-sm')} Clear</button>
          </div>
        </div>
        <div class="console inst-console" data-console></div>
        <div class="inst-console-empty" data-empty hidden></div>
      </div>`,
  );

  const consoleEl = pane.querySelector('[data-console]');
  const emptyEl = pane.querySelector('[data-empty]');
  const statusEl = pane.querySelector('[data-status]');
  const jumpBtn = pane.querySelector('[data-jump]');
  const loadBtn = pane.querySelector('[data-load-log]');

  const lines = () => store.state.console[id] ?? [];

  function drawStatus() {
    const running = isRunning(id);
    setHtml(
      statusEl,
      html`
        <span class="small muted">${running ? 'Live output from the running game' : 'Output from the most recent launch'}</span>
        ${running && html`<span class="row small success-text" style="gap:6px"><span class="live-dot"></span> live</span>`}
        <span class="tiny faint">${plural(consoleEl.childElementCount, 'line')}</span>`,
    );
  }

  function drawEmpty() {
    const empty = consoleEl.childElementCount === 0;
    consoleEl.hidden = empty;
    emptyEl.hidden = !empty;
    if (empty) {
      setHtml(
        emptyEl,
        loadingLog
          ? html`<div class="loading-block">${spinner()} Reading the last log…</div>`
          : html`<div class="empty">
              <div class="empty-icon">${icon('terminal', 'icon-lg')}</div>
              <h3>Nothing here yet</h3>
              <p>Launch the game and everything it prints streams in live. You can also load the log from the last launch.</p>
            </div>`,
      );
    }
  }

  function scrollToEnd() {
    consoleEl.scrollTop = consoleEl.scrollHeight;
  }

  /** Bring the DOM in step with the store: append new lines, or rebuild after a reset. */
  function sync() {
    const all = lines();
    let start = 0;
    if (lastRendered) {
      const at = all.lastIndexOf(lastRendered);
      if (at >= 0) start = at + 1;
      else consoleEl.replaceChildren(); // cleared, replaced or rolled past the cap
    } else {
      consoleEl.replaceChildren();
    }
    if (start < all.length) {
      const fragment = document.createDocumentFragment();
      for (let i = start; i < all.length; i++) fragment.appendChild(lineNode(all[i]));
      consoleEl.appendChild(fragment);
    }
    lastRendered = all[all.length - 1] ?? null;
    while (consoleEl.childElementCount > CONSOLE_LIMIT) consoleEl.firstElementChild.remove();
    drawEmpty();
    drawStatus();
    if (follow) scrollToEnd();
  }

  async function loadLastLog({ quiet = false } = {}) {
    if (loadingLog) return;
    loadingLog = true;
    loadBtn.disabled = true;
    drawEmpty();
    try {
      const text = await api.launch.logs(id, 1000);
      if (!ctx.alive()) return;
      if (!text) {
        if (!quiet) toast('No log from a previous launch yet.', { kind: 'info' });
      } else {
        // While a game is running its live output is the truth; don't replace it.
        if (isRunning(id) && lines().length) {
          if (!quiet) toast('The game is running, so its live output is shown instead.', { kind: 'info' });
        } else {
          const parsed = text
            .split(/\r?\n/)
            .filter((line) => line.length > 0)
            .slice(-CONSOLE_LIMIT)
            .map((line) => ({ stream: 'stdout', text: line }));
          store.state.console[id] = parsed;
          follow = true;
          store.set('console', store.state.console);
        }
      }
    } catch (error) {
      // A first launch has no log yet.
      if (!quiet && ctx.alive()) toastError(error, "Couldn't read the last log");
    }
    loadingLog = false;
    if (!ctx.alive()) return;
    loadBtn.disabled = false;
    drawEmpty();
  }

  consoleEl.addEventListener('scroll', () => {
    const fromBottom = consoleEl.scrollHeight - consoleEl.scrollTop - consoleEl.clientHeight;
    follow = fromBottom < FOLLOW_THRESHOLD;
    jumpBtn.hidden = follow;
  });

  pane.addEventListener('click', async (event) => {
    const t = event.target;
    if (t.closest('[data-jump]')) {
      follow = true;
      jumpBtn.hidden = true;
      return scrollToEnd();
    }
    if (t.closest('[data-load-log]')) return loadLastLog();
    if (t.closest('[data-clear]')) {
      store.state.console[id] = [];
      return store.set('console', store.state.console);
    }
    if (t.closest('[data-copy]')) {
      const text = lines().map((line) => line.text).join('\n');
      if (!text) return toast('The console is empty.', { kind: 'info' });
      try {
        await navigator.clipboard.writeText(text);
        toast(`Copied ${plural(lines().length, 'line')}`, { kind: 'success', duration: 2000 });
      } catch (error) {
        toastError(error, "Couldn't copy to the clipboard");
      }
    }
  });

  const offConsole = store.on('console', sync);
  const offRunning = store.on('running', drawStatus);
  sync();
  // Nothing streamed yet: show the last launch's log, which survives a crash.
  if (!lines().length) loadLastLog({ quiet: true });

  return () => {
    offConsole();
    offRunning();
  };
}
