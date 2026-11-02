// Play: your instances as planets on one orbit. The selected one sits in front with the
// bracketed LAUNCH under it; the side panel lists every instance as a telemetry row and
// shows the selected one's numbers. Orbit Client is always on the orbit, installed or not.
//
// The planets are built once and kept: selecting another world only moves them. Each world
// is a fixed-size box placed with a transform, so a spin is a GPU-composited glide from
// wherever the world is right now (even mid-spin) to its new slot, and the planet artwork is
// never redrawn while it moves. Worlds that wrap around a short orbit leave off one end and
// come back in from the other instead of cutting across the front.
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

/** Every world box is this big; slots scale it down. */
const BASE = 176;

/** Slots on the orbit, by distance from the front: where they sit (% of the stage) and how big. */
const SLOTS = {
  '-2': { x: 12, y: 63, size: 56 },
  '-1': { x: 28, y: 52, size: 78 },
  0: { x: 50, y: 42, size: BASE },
  1: { x: 72, y: 52, size: 78 },
  2: { x: 88, y: 63, size: 56 },
};

/** Just past either end of the orbit, where worlds wait unseen. */
const offOrbit = (side) => ({ x: side < 0 ? 3 : 97, y: 71, size: 34 });

const slotFor = (offset) => (Math.abs(offset) <= 2 ? SLOTS[offset] : offOrbit(offset));

const SPIN = { duration: 320, easing: 'cubic-bezier(.25,.8,.25,1)' };

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

/** Each world's place relative to the front, the short way round: 0 is selected, -1 left of it. */
function offsets(list, index) {
  const n = list.length;
  const half = Math.floor(n / 2);
  return new Map(list.map((instance, i) => [instance.id, ((((i - index) % n) + n + half) % n) - half]));
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
  setHtml(
    root,
    html`<div class="play">
      <section class="play-stage" data-stage>
        <div class="play-title" data-title></div>
        <div class="play-orbit far" aria-hidden="true"></div>
        <div class="play-orbit" aria-hidden="true"></div>
        <div class="world-layer" data-worlds></div>
        <button class="play-arrow glass" data-spin="-1" aria-label="Previous instance">${icon('chevron-left')}</button>
        <button class="play-arrow glass is-next" data-spin="1" aria-label="Next instance">${icon('chevron-right')}</button>
        <div data-launch></div>
      </section>
      <aside class="play-side" data-side></aside>
    </div>`,
  );
  const stage = root.querySelector('[data-stage]');
  const layer = root.querySelector('[data-worlds]');
  const titleEl = root.querySelector('[data-title]');
  const launchEl = root.querySelector('[data-launch]');
  const sideEl = root.querySelector('[data-side]');

  // ---------- The worlds ----------

  /** Per world: its box, its label, and the offset it was last placed at. */
  const placed = new Map();

  const placement = (offset) => {
    const slot = slotFor(offset);
    const cx = (slot.x / 100) * stage.clientWidth;
    const cy = (slot.y / 100) * stage.clientHeight;
    const visible = Math.abs(offset) <= 2;
    return {
      world: { transform: `translate(${cx - BASE / 2}px, ${cy - BASE / 2}px) scale(${slot.size / BASE})`, opacity: visible ? 1 : 0 },
      label: { transform: `translate(${cx - 80}px, ${cy + slot.size / 2 + 6}px)`, opacity: visible && offset !== 0 ? 1 : 0 },
    };
  };

  const createWorld = (instance) => {
    const world = document.createElement('button');
    world.className = 'world';
    world.dataset.world = instance.id;
    world.innerHTML = `<span class="world-body"></span><span class="world-live" aria-label="Running" hidden></span>`;
    const label = document.createElement('span');
    label.className = 'world-label';
    label.setAttribute('aria-hidden', 'true');
    layer.append(world, label);
    const entry = { world, label, offset: null, kind: null };
    placed.set(instance.id, entry);
    return entry;
  };

  /** Move an element to `to`, gliding from where it is right now (mid-animation included). */
  const glide = (el, to, via) => {
    const now = getComputedStyle(el);
    const from = { transform: now.transform === 'none' ? to.transform : now.transform, opacity: now.opacity };
    el.getAnimations().forEach((a) => a.cancel());
    Object.assign(el.style, to);
    el.animate(via ? [from, ...via, to] : [from, to], SPIN);
  };

  /** Lay the worlds out for the current selection; animated unless it's the first layout or a resize. */
  const syncWorlds = (list, index, animate) => {
    const offs = offsets(list, index);
    const n = list.length;
    for (const [id, entry] of placed) {
      if (!offs.has(id)) {
        entry.world.remove();
        entry.label.remove();
        placed.delete(id);
      }
    }
    for (const instance of list) {
      const entry = placed.get(instance.id) ?? createWorld(instance);
      const offset = offs.get(instance.id);
      const kind = planetOf(instance).kind.id;
      if (entry.kind !== kind) {
        entry.world.querySelector('.world-body').innerHTML = String(planet(instance));
        entry.kind = kind;
      }
      entry.world.classList.toggle('is-front', offset === 0);
      entry.world.classList.toggle('is-hidden', Math.abs(offset) > 2);
      entry.world.tabIndex = Math.abs(offset) > 2 ? -1 : 0;
      entry.world.setAttribute('aria-label', `${instance.name}${offset === 0 ? ' (selected)' : ''}`);
      entry.world.querySelector('.world-live').hidden = !store.state.running.has(instance.id);
      setHtml(entry.label, html`<b>${instance.name}</b>${instance.gameVersion}`);

      const to = placement(offset);
      const moved = entry.offset !== null && entry.offset !== offset;
      if (!animate || !moved) {
        entry.world.getAnimations().forEach((a) => a.cancel());
        entry.label.getAnimations().forEach((a) => a.cancel());
        Object.assign(entry.world.style, to.world);
        Object.assign(entry.label.style, to.label);
      } else if (Math.abs(offset - entry.offset) > n / 2) {
        // Wrapped round a short orbit: out past the end it was nearest, back in from the other.
        const out = placement(Math.sign(entry.offset) * 3);
        const back = placement(Math.sign(offset) * 3);
        glide(entry.world, to.world, [{ ...out.world, offset: 0.5 }, { ...back.world, offset: 0.5 }]);
        glide(entry.label, to.label, [{ ...out.label, offset: 0.5 }, { ...back.label, offset: 0.5 }]);
      } else {
        glide(entry.world, to.world);
        glide(entry.label, to.label);
      }
      entry.offset = offset;
    }
  };

  // ---------- Everything around them ----------

  const draw = (animate = false) => {
    const list = worlds();
    const index = selectedIndex(list);
    const selected = list[index];
    const running = store.state.running.has(selected.id);
    const launching = store.state.launching.has(selected.id) || (selected.placeholder && store.state.orbitInstalling);
    const server = featuredServer();
    const status = running ? 'RUNNING' : launching ? 'STARTING' : selected.placeholder ? 'NOT INSTALLED' : 'READY';
    const titleChanged = titleEl.dataset.id !== selected.id;

    titleEl.dataset.id = selected.id;
    setHtml(
      titleEl,
      html`<small>INSTANCE ${index + 1} / ${list.length}</small>
        <h1 class="truncate">${selected.name}</h1>
        <p>Minecraft ${selected.gameVersion} · ${loaderLabel(selected.loader)}${selected.placeholder
          ? ' · Install to play'
          : html` · ${selected.modCount} ${selected.modCount === 1 ? 'mod' : 'mods'}`}</p>`,
    );
    if (animate && titleChanged) titleEl.animate([{ opacity: 0.25 }, { opacity: 1 }], { duration: 200, easing: 'ease-out' });

    root.querySelectorAll('[data-spin]').forEach((arrow) => (arrow.hidden = list.length < 2));

    setHtml(
      launchEl,
      html`<div class="launch-row">
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
        html`<button class="join-link" data-join ${launching ? 'disabled' : ''}>${icon('server', 'icon-sm')} Launch into ${server}</button>`}`,
    );

    const listScroll = sideEl.querySelector('.tele-list')?.scrollTop ?? 0;
    setHtml(
      sideEl,
      html`<section class="side-panel glass">
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
        </section>`,
    );
    const listEl = sideEl.querySelector('.tele-list');
    if (listEl) listEl.scrollTop = listScroll;

    syncWorlds(list, index, animate);
  };

  // ---------- Actions ----------

  const select = (id) => {
    if (prefs.get('play.selected', null) === id) return;
    prefs.set('play.selected', id);
    draw(true);
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

  /**
   * Turn a world into another kind: it collapses into a flash, a shockwave rings out, and the
   * new world swells back up with a little overshoot.
   */
  const morphPlanet = async (id, kindId) => {
    const entry = placed.get(id);
    const body = entry?.world.querySelector('.world-body');
    if (body) {
      await body
        .animate([{ transform: 'scale(1)', filter: 'brightness(1)' }, { transform: 'scale(0.15)', filter: 'brightness(3)', opacity: 0.2 }], {
          duration: 160,
          easing: 'cubic-bezier(.55,0,.8,.4)',
          fill: 'forwards',
        })
        .finished.catch(() => {});
    }
    setPlanetKind(id, kindId);
    draw();
    if (!body) return;
    body.getAnimations().forEach((a) => a.cancel());
    body.animate(
      [
        { transform: 'scale(0.15)', filter: 'brightness(3)', opacity: 0.2 },
        { transform: 'scale(1.12)', filter: 'brightness(1.3)', opacity: 1, offset: 0.65 },
        { transform: 'scale(1)', filter: 'brightness(1)', opacity: 1 },
      ],
      { duration: 340, easing: 'cubic-bezier(.2,.8,.25,1)' },
    );
    const burst = document.createElement('span');
    burst.className = 'world-burst';
    burst.style.setProperty('--glow', PLANET_KINDS.find((k) => k.id === kindId)?.atmo ?? '#fff');
    entry.world.append(burst);
    burst
      .animate([{ transform: 'scale(0.3)', opacity: 0.95 }, { transform: 'scale(1.9)', opacity: 0 }], { duration: 450, easing: 'cubic-bezier(.1,.7,.3,1)' })
      .finished.finally(() => burst.remove());
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
  stage.addEventListener(
    'wheel',
    (event) => {
      if (Math.abs(event.deltaY) < 4) return;
      event.preventDefault();
      const now = performance.now();
      if (now - wheelLock < 160) return;
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

  // Slots are fractions of the stage: re-place (without gliding) when it changes size.
  const resize = new ResizeObserver(() => {
    const list = worlds();
    syncWorlds(list, selectedIndex(list), false);
  });
  resize.observe(stage);

  draw();
  const off = store.on(['instances', 'running', 'launching', 'settings', 'orbitInstalling'], () => draw());
  return () => {
    off?.();
    resize.disconnect();
    document.removeEventListener('keydown', onKey);
  };
}
