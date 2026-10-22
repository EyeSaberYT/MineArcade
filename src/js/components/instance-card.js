// Instance card (grid) and row (list) shared by the Home and Library pages.
// Render with `instanceCard(summary)` / `instanceRow(summary)`, then call
// `wireInstanceList(container, { onScan })` once to handle clicks.
import { joinFeaturedServer, launchInstance, stopInstance, featuredServer } from '../actions.js';
import { navigate } from '../router.js';
import { store } from '../store.js';
import { html, icon, instanceIcon, loaderBadge, relativeTime, spinner } from '../ui.js';

function playButton(instance, extraClass = '') {
  const running = store.state.running.has(instance.id);
  const launching = store.state.launching.has(instance.id);
  if (running) {
    return html`<button class="btn btn-sm btn-danger ${extraClass}" data-action="stop" title="Force-close the game">${icon('square')} Stop</button>`;
  }
  return html`<button class="btn btn-sm btn-play ${extraClass}" data-action="play" ${launching ? 'disabled' : ''}>
    ${launching ? spinner() : icon('play')} ${launching ? 'Starting…' : 'Play'}
  </button>`;
}

const mods = (count) => `${count} ${count === 1 ? 'mod' : 'mods'}`;

export function instanceCard(instance, { scan = false } = {}) {
  const running = store.state.running.has(instance.id);
  const server = featuredServer();
  return html`
    <article class="inst-card" data-instance="${instance.id}" tabindex="0">
      <div class="row" style="align-items:flex-start">
        ${instanceIcon(instance, 48)}
        <div class="grow">
          <div class="row" style="gap:7px">
            <strong class="truncate">${instance.name}</strong>
            ${running && html`<span class="live-dot" title="Running"></span>`}
          </div>
          <div class="row-wrap" style="gap:6px;margin-top:8px">
            <span class="badge badge-mono">${instance.gameVersion}</span>
            ${loaderBadge(instance.loader)}
          </div>
        </div>
      </div>
      <div class="inst-card-foot">
        <span>${mods(instance.modCount)}</span>
        <span class="truncate">${relativeTime(instance.lastPlayed)}</span>
      </div>
      <div class="row" style="gap:6px">
        ${playButton(instance, 'grow')}
        ${server && !running && html`<button class="btn btn-icon btn-sm" data-action="join" title="Join ${server}">${icon('server')}</button>`}
        ${scan && html`<button class="btn btn-icon btn-sm" data-action="scan" title="Scan mods and packs">${icon('shield')}</button>`}
      </div>
    </article>`;
}

export function instanceRow(instance, { scan = false } = {}) {
  const running = store.state.running.has(instance.id);
  return html`
    <div class="list-row inst-row" data-instance="${instance.id}" tabindex="0">
      ${instanceIcon(instance, 36)}
      <strong class="grow truncate">${instance.name}</strong>
      ${running && html`<span class="live-dot" title="Running"></span>`}
      <span class="badge badge-mono">${instance.gameVersion}</span>
      ${loaderBadge(instance.loader)}
      <span class="small muted nowrap" style="width:70px;text-align:right">${mods(instance.modCount)}</span>
      <span class="small muted nowrap" style="width:120px;text-align:right">${relativeTime(instance.lastPlayed)}</span>
      <div class="row-actions">
        ${scan && html`<button class="btn btn-icon btn-sm" data-action="scan" title="Scan mods and packs">${icon('shield')}</button>`}
        ${playButton(instance)}
      </div>
    </div>`;
}

/** Handle clicks on cards/rows inside `container` (call once per container). */
export function wireInstanceList(container, { onScan } = {}) {
  const findInstance = (target) => {
    const node = target.closest('[data-instance]');
    return node ? store.state.instances.find((i) => i.id === node.dataset.instance) : null;
  };

  container.addEventListener('click', (event) => {
    const instance = findInstance(event.target);
    if (!instance) return;
    const action = event.target.closest('[data-action]')?.dataset.action;
    if (action === 'play') return launchInstance(instance.id);
    if (action === 'stop') return stopInstance(instance.id, instance.name);
    if (action === 'join') return joinFeaturedServer(instance.id);
    if (action === 'scan') return onScan?.(instance);
    if (!action) navigate(`/instance/${instance.id}`);
  });

  container.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter' || event.target.closest('button')) return;
    const instance = findInstance(event.target);
    if (instance) navigate(`/instance/${instance.id}`);
  });
}
