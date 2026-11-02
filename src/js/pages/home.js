// Play: your instances as planets on one orbit. The selected one sits in front with the
// bracketed LAUNCH under it; the side panel lists every instance as a telemetry row and
// shows the selected one's numbers. Orbit Client is always on the orbit, installed or not.
import {
  featuredServer,
  launchInstance,
  orbitInstance,
  playOrbitClient,
  spotlightInstance,
  stopInstance,
} from '../actions.js';
import { PLANET_KINDS, planet, planetOf, setPlanetKind } from '../components/planet.js';
import { openNewInstanceDialog } from '../dialogs/new-instance.js';
import { revealInstanceFolder } from './instance/shared.js';
import { navigate } from '../router.js';
import { prefs, store } from '../store.js';
import { formatDuration, html, icon, loaderLabel, relativeTime, setHtml, showMenu, spinner } from '../ui.js';

/** Stands in for Orbit Client before its instance exists: launching it installs it. */
const ORBIT_PLACEHOLDER = {
  id: '__orbit-client',
  name: 'Orbit Client',
  gameVersion: '1.21.11',
  loader: 'fabric',
  modCount: 0,
  tags: ['orbit-client'],
  placeholder: true,
};

/** Slots on the orbit, by distance from the front: where they sit (% of the stage) and how big. */
const SLOTS = {
  '-2': { x: 12, y: 63, size: 56 },
  '-1': { x: 28, y: 52, size: 78 },
  0: { x: 50, y: 42, size: 176 },
  1: { x: 72, y: 52, size: 78 },
  2: { x: 88, y: 63, size: 56 },
};

/** Just past either end of the orbit: where worlds fade in from and out to while it spins. */
const OFF_ORBIT = { x: 3, y: 71, size: 34 };

/** A point anywhere along the orbit, between the slots: offset 0.5 is halfway from the front to the next world. */
function orbitAt(offset) {
  const slot = (n) => (Math.abs(n) >= 3 ? { ...OFF_ORBIT, x: n < 0 ? OFF_ORBIT.x : 100 - OFF_ORBIT.x } : SLOTS[n]);
  const t = Math.max(-3, Math.min(3, offset));
  const lo = Math.floor(t);
  const hi = Math.ceil(t);
  if (lo === hi) return slot(lo);
  const a = slot(lo);
  const b = slot(hi);
  const f = t - lo;
  return { x: a.x + (b.x - a.x) * f, y: a.y + (b.y - a.y) * f, size: a.size + (b.size - a.size) * f };
}

const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;

const SHORT_LOADER = { fabric: 'FAB', quilt: 'QLT', forge: 'FRG', neoforge: 'NEO', vanilla: 'VAN' };

/** Orbit Client first, then the rest by last played. */
function worlds() {
  const list = [...store.state.instances].sort((a, b) => {
    const diff = new Date(b.lastPlayed ?? 0) - new Date(a.lastPlayed ?? 0);
    return diff || a.name.localeCompare(b.name);
  });
  const orbit = orbitInstance();
  if (orbit) return [orbit, ...list.filter((i) => i.id !== orbit.id)];
  return [ORBIT_PLACEHOLDER, ...list];
}

function selectedIndex(list) {
  const id = prefs.get('play.selected', null) ?? spotlightInstance()?.id;
  return Math.max(0, list.findIndex((i) => i.id === id));
}

/** "2h", "3d", "2w": short enough for a telemetry column. */
function ago(iso) {
  if (!iso) return 'never';
  const minutes = Math.max(0, (Date.now() - new Date(iso)) / 60000);
  if (minutes < 60) return `${Math.round(minutes)}m`;
  if (minutes < 1440) return `${Math.round(minutes / 60)}h`;
  if (minutes < 10080) return `${Math.round(minutes / 1440)}d`;
  if (minutes < 43200) return `${Math.round(minutes / 10080)}w`;
  return `${Math.round(minutes / 43200)}mo`;
}

const memoryGb = (instance) => (instance.settings?.memoryMax ?? 4096) / 1024;

export function render(root) {
  // Edge to edge: the orbit uses the whole page area, not the padded column other pages use.
  root.classList.add('page-play');
  const draw = () => {
    const list = worlds();
    const index = selectedIndex(list);
    const selected = list[index];
    const running = store.state.running.has(selected.id);
    const launching = store.state.launching.has(selected.id) || (selected.placeholder && store.state.orbitInstalling);
    const server = featuredServer();

    // The worlds around the selected one, without showing any twice on a short orbit.
    const reach = Math.min(2, Math.floor((list.length - 1) / 2));
    const extra = list.length - 1 - reach * 2 > 0 && reach < 2 ? 1 : 0;
    const shown = [];
    for (let offset = -reach; offset <= reach + extra; offset++) {
      shown.push({ offset, instance: list[(index + offset + list.length) % list.length] });
    }

    const status = running ? 'RUNNING' : launching ? 'STARTING' : selected.placeholder ? 'NOT INSTALLED' : 'READY';

    setHtml(
      root,
      html`<div class="play">
        <section class="play-stage" data-stage>
          <div class="play-title">
            <small>INSTANCE ${index + 1} / ${list.length}</small>
            <h1 class="truncate">${selected.name}</h1>
            <p>Minecraft ${selected.gameVersion} · ${loaderLabel(selected.loader)}${selected.placeholder
              ? ' · Install to play'
              : html` · ${selected.modCount} ${selected.modCount === 1 ? 'mod' : 'mods'}`}</p>
          </div>
          <div class="play-orbit far" aria-hidden="true"></div>
          <div class="play-orbit" aria-hidden="true"></div>

          ${shown.map(({ offset, instance }) => {
            const slot = SLOTS[offset];
            const live = store.state.running.has(instance.id);
            return html`<button class="world ${offset === 0 ? 'is-front' : ''}" data-world="${instance.id}" data-offset="${offset}"
                style="left:${slot.x}%;top:${slot.y}%;--s:${slot.size}px"
                aria-label="${instance.name}${offset === 0 ? ' (selected)' : ''}">
                ${planet(instance)}
                ${live && html`<span class="world-live" aria-label="Running"></span>`}
              </button>
              ${offset !== 0 &&
              html`<span class="world-label" style="left:${slot.x}%;top:calc(${slot.y}% + ${slot.size / 2 + 6}px)">
                <b>${instance.name}</b>${instance.gameVersion}
              </span>`}`;
          })}

          ${list.length > 1 &&
          html`<button class="play-arrow glass" data-spin="-1" aria-label="Previous instance">${icon('chevron-left')}</button>
            <button class="play-arrow glass is-next" data-spin="1" aria-label="Next instance">${icon('chevron-right')}</button>`}

          <div class="launch-row">
            <div class="readout">STATUS <b class="${running ? 'is-live' : launching ? 'is-busy' : selected.placeholder ? 'is-warn' : ''}">${status}</b><br>
              ${selected.placeholder ? 'SETS UP FABRIC + API' : `MODS ${selected.modCount}`}</div>
            ${running
              ? html`<button class="launch-btn is-stop" data-stop="${selected.id}">STOP</button>`
              : html`<button class="launch-btn" data-launch ${launching ? 'disabled' : ''}>
                  ${launching ? spinner() : ''}${selected.placeholder ? 'INSTALL' : 'LAUNCH'}
                </button>`}
            <div class="readout">MEM ${selected.placeholder ? '4.0' : memoryGb(selected).toFixed(1)} GB<br>
              LAST ${selected.lastPlayed ? relativeTime(selected.lastPlayed).toUpperCase() : 'NEVER'}</div>
          </div>
          ${server && !running &&
          html`<button class="join-link" data-join ${launching ? 'disabled' : ''}>${icon('server', 'icon-sm')} Launch into ${server}</button>`}
        </section>

        <aside class="play-side">
          <section class="side-panel glass">
            <div class="side-head"><h2>Instances</h2><span>${store.state.instances.length} TOTAL</span></div>
            <div class="tele-row tele-hdr"><span></span><span>NAME</span><span>VERSION</span><span>MODS</span><span>LAST</span></div>
            <div class="tele-list">
              ${list.map((instance) => {
                const { kind } = planetOf(instance);
                return html`<button class="tele-row ${instance.id === selected.id ? 'is-on' : ''}" data-select="${instance.id}">
                  <span class="tele-dot" style="background:${kind.atmo}"></span>
                  <span class="tele-name truncate">${instance.name}</span>
                  <span>${instance.gameVersion} ${SHORT_LOADER[instance.loader] ?? ''}</span>
                  <span>${instance.placeholder ? '–' : instance.modCount}</span>
                  <span class="tele-last">${instance.placeholder ? 'new' : ago(instance.lastPlayed)}</span>
                </button>`;
              })}
            </div>
            <button class="tele-new" data-new>${icon('plus', 'icon-sm')} New instance</button>
          </section>

          <section class="side-panel glass">
            <div class="side-head"><h2>Telemetry</h2><span class="truncate">${selected.name.toUpperCase()}</span></div>
            <div class="tele-grid">
              <div class="tele-card"><small>PLAYTIME</small><b>${selected.playTimeSecs ? formatDuration(selected.playTimeSecs) : '0m'}</b></div>
              <div class="tele-card"><small>MODS</small><b>${selected.placeholder ? '–' : selected.modCount}</b></div>
              <div class="tele-card"><small>LOADER</small><b>${loaderLabel(selected.loader)}</b></div>
              <div class="tele-card"><small>VERSION</small><b>${selected.gameVersion}</b></div>
            </div>
            <div class="tele-mem">MEMORY ${memoryGb(selected).toFixed(1)} / 16 GB
              <div class="tele-seg">${Array.from({ length: 16 }, (_, i) => html`<i class="${i < Math.round(memoryGb(selected)) ? 'on' : ''}"></i>`)}</div>
            </div>
            ${!selected.placeholder &&
            html`<div class="side-actions">
              <button class="btn btn-ghost btn-sm" data-go="/instance/${selected.id}">${icon('settings', 'icon-sm')} Manage</button>
              <button class="btn btn-ghost btn-sm" data-folder="${selected.id}">${icon('folder', 'icon-sm')} Folder</button>
            </div>`}
          </section>
        </aside>
      </div>`,
    );
  };

  /**
   * Spin the orbit: each world glides along it from where it was to its new place (through the
   * slots in between, so it follows the ellipse rather than cutting across). Worlds arriving fade
   * in from the end of the orbit they come from, and the title fades over.
   */
  const animateOrbit = (before, shift) => {
    const stage = root.querySelector('[data-stage]');
    if (!stage || !shift || reducedMotion()) return;
    const width = stage.clientWidth;
    const height = stage.clientHeight;
    const timing = { duration: 460, easing: 'cubic-bezier(.22,.8,.24,1)' };
    stage.querySelectorAll('[data-world]').forEach((world) => {
      const to = Number(world.dataset.offset);
      let from = before.get(world.dataset.world);
      // A world that wrapped around a short orbit, or wasn't shown, arrives from the far end instead.
      const arriving = from === undefined || from - shift !== to;
      if (arriving) from = to + Math.sign(shift) * Math.max(1, 3 - Math.abs(to));
      const end = orbitAt(to);
      const frames = [];
      for (let step = 0; step <= 8; step++) {
        const at = orbitAt(from + ((to - from) * step) / 8);
        const dx = ((at.x - end.x) / 100) * width;
        const dy = ((at.y - end.y) / 100) * height;
        frames.push({
          transform: `translate(calc(-50% + ${dx}px), calc(-50% + ${dy}px)) scale(${at.size / end.size})`,
          opacity: arriving ? step / 8 : 1,
        });
      }
      world.animate(frames, timing);
    });
    stage.querySelectorAll('.world-label').forEach((label) =>
      label.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 260, delay: 240, fill: 'backwards' }),
    );
    stage.querySelector('.play-title')?.animate(
      [{ opacity: 0, transform: `translateX(${shift > 0 ? 18 : -18}px)` }, { opacity: 1, transform: 'none' }],
      { duration: 320, easing: 'ease-out' },
    );
  };

  /**
   * Turn a world into another kind: it collapses into a flash, a shockwave rings out, and the
   * new world swells back up with a little overshoot. Worlds off the orbit just change.
   */
  const morphPlanet = async (id, kindId) => {
    const findWorld = () => root.querySelector(`[data-world="${CSS.escape(id)}"]`);
    const old = findWorld();
    if (old && !reducedMotion()) {
      await old.animate(
        [
          { transform: 'translate(-50%, -50%) scale(1)', filter: 'brightness(1)' },
          { transform: 'translate(-50%, -50%) scale(0.15)', filter: 'brightness(3)', opacity: 0.2 },
        ],
        { duration: 240, easing: 'cubic-bezier(.55,0,.8,.4)', fill: 'forwards' },
      ).finished.catch(() => {});
    }
    setPlanetKind(id, kindId);
    draw();
    const world = findWorld();
    if (!world || reducedMotion()) return;
    world.animate(
      [
        { transform: 'translate(-50%, -50%) scale(0.15)', filter: 'brightness(3)', opacity: 0.2 },
        { transform: 'translate(-50%, -50%) scale(1.12)', filter: 'brightness(1.3)', opacity: 1, offset: 0.65 },
        { transform: 'translate(-50%, -50%) scale(1)', filter: 'brightness(1)', opacity: 1 },
      ],
      { duration: 520, easing: 'cubic-bezier(.2,.8,.25,1)' },
    );
    // The shockwave: a ring in the new world's atmosphere colour, sized to the world.
    const burst = document.createElement('span');
    burst.className = 'world-burst';
    burst.style.cssText = `left:${world.style.left};top:${world.style.top};--s:${world.style.getPropertyValue('--s')};--glow:${planetOf({ id, tags: [] }).kind.atmo}`;
    world.after(burst);
    burst
      .animate(
        [
          { transform: 'translate(-50%, -50%) scale(0.3)', opacity: 0.95 },
          { transform: 'translate(-50%, -50%) scale(1.9)', opacity: 0 },
        ],
        { duration: 650, easing: 'cubic-bezier(.1,.7,.3,1)' },
      )
      .finished.finally(() => burst.remove());
  };

  const select = (id) => {
    // Where every world sits now, so the redraw can animate from there.
    const before = new Map([...root.querySelectorAll('[data-world]')].map((w) => [w.dataset.world, Number(w.dataset.offset)]));
    let shift = before.get(id);
    if (shift === undefined) {
      // Picked from the list while off the orbit: spin the short way round to it.
      const list = worlds();
      const n = list.length;
      const diff = list.findIndex((i) => i.id === id) - selectedIndex(list);
      shift = ((((diff % n) + n + Math.floor(n / 2)) % n) - Math.floor(n / 2)) || 0;
    }
    prefs.set('play.selected', id);
    draw();
    animateOrbit(before, shift);
  };

  const spin = (direction) => {
    const list = worlds();
    if (list.length < 2) return;
    select(list[(selectedIndex(list) + direction + list.length) % list.length].id);
  };

  const launchSelected = (quickPlay) => {
    const list = worlds();
    const selected = list[selectedIndex(list)];
    if (selected.tags?.includes('orbit-client')) return playOrbitClient(quickPlay);
    return launchInstance(selected.id, quickPlay);
  };

  root.addEventListener('click', (event) => {
    const target = event.target;
    const go = target.closest('[data-go]');
    if (go) return navigate(go.dataset.go);
    if (target.closest('[data-new]')) return openNewInstanceDialog();
    const spinBtn = target.closest('[data-spin]');
    if (spinBtn) return spin(Number(spinBtn.dataset.spin));
    const world = target.closest('[data-world]');
    if (world) return world.classList.contains('is-front') ? launchSelected() : select(world.dataset.world);
    const row = target.closest('[data-select]');
    if (row) return select(row.dataset.select);
    if (target.closest('[data-launch]')) return launchSelected();
    if (target.closest('[data-join]')) return launchSelected({ type: 'multiplayer', address: featuredServer() });
    const stop = target.closest('[data-stop]');
    if (stop) {
      const instance = store.state.instances.find((i) => i.id === stop.dataset.stop);
      return stopInstance(stop.dataset.stop, instance?.name);
    }
    const folder = target.closest('[data-folder]');
    if (folder) return revealInstanceFolder(folder.dataset.folder);
  });

  // Right-click a world (or its row) to play it, open it, or change what kind of world it is.
  root.addEventListener('contextmenu', (event) => {
    const node = event.target.closest('[data-world], [data-select]');
    const id = node?.dataset.world ?? node?.dataset.select;
    const instance = id && store.state.instances.find((i) => i.id === id);
    if (!instance) return;
    event.preventDefault();
    const current = planetOf(instance).kind.id;
    showMenu(node, [
      { header: instance.name },
      { label: 'Play', icon: 'play', onSelect: () => launchInstance(instance.id) },
      { label: 'Select', onSelect: () => select(instance.id) },
      { label: 'View instance', icon: 'eye', onSelect: () => navigate(`/instance/${instance.id}`) },
      { separator: true },
      { header: 'Planet' },
      ...PLANET_KINDS.map((kind) => ({
        label: kind.label,
        trail: kind.id === current ? 'Current' : '',
        onSelect: () => morphPlanet(instance.id, kind.id),
      })),
    ]);
  });

  // Scroll or arrow keys spin the orbit.
  let wheelLock = 0;
  root.addEventListener(
    'wheel',
    (event) => {
      if (!event.target.closest('[data-stage]') || Math.abs(event.deltaY) < 4) return;
      event.preventDefault();
      const now = performance.now();
      if (now - wheelLock < 220) return;
      wheelLock = now;
      spin(event.deltaY > 0 ? 1 : -1);
    },
    { passive: false },
  );
  const onKey = (event) => {
    if (event.target.closest('input, textarea, select, [contenteditable]') || document.querySelector('.modal')) return;
    if (event.key === 'ArrowLeft') spin(-1);
    if (event.key === 'ArrowRight') spin(1);
  };
  document.addEventListener('keydown', onKey);

  draw();
  const off = store.on(['instances', 'running', 'launching', 'settings', 'orbitInstalling'], draw);
  return () => {
    off?.();
    document.removeEventListener('keydown', onKey);
  };
}
