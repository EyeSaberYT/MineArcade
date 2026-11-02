// Home: the Orbit Client hero, the MineArcade server hero, "continue playing", stats and recent instances.
import {
  featuredServer,
  joinFeaturedServer,
  launchInstance,
  orbitInstance,
  playOrbitClient,
  spotlightInstance,
  stopInstance,
} from '../actions.js';
import { instanceCard, wireInstanceList } from '../components/instance-card.js';
import { openNewInstanceDialog } from '../dialogs/new-instance.js';
import { navigate } from '../router.js';
import { prefs, store } from '../store.js';
import {
  emptyState,
  formatDuration,
  html,
  icon,
  instanceIcon,
  loaderBadge,
  loaderLabel,
  relativeTime,
  setHtml,
  showMenu,
  spinner,
} from '../ui.js';

/** The instance the hero's Join button uses: the one picked with ▾, else the most recent. */
function heroInstance() {
  const picked = prefs.get('home.heroInstance', null);
  return store.state.instances.find((i) => i.id === picked) ?? spotlightInstance();
}

export function render(root) {
  const draw = () => {
    const instances = store.state.instances;
    const spotlight = spotlightInstance();
    const hero = heroInstance();
    const server = featuredServer();
    const running = spotlight && store.state.running.has(spotlight.id);
    const launching = spotlight && store.state.launching.has(spotlight.id);
    const heroRunning = hero && store.state.running.has(hero.id);
    const heroLaunching = hero && store.state.launching.has(hero.id);
    const runningCount = instances.filter((i) => store.state.running.has(i.id)).length;
    const modCount = instances.reduce((sum, i) => sum + i.modCount, 0);
    const guideDismissed = prefs.get('home.guideDismissed', false);
    const orbit = orbitInstance();
    const orbitRunning = orbit && store.state.running.has(orbit.id);
    const orbitBusy = store.state.orbitInstalling || (orbit && store.state.launching.has(orbit.id));

    setHtml(
      root,
      html`
        <header class="page-head">
          <div>
            <h1>Ready, player one</h1>
            <p>Everything you need to get back into the game.</p>
          </div>
          <div class="page-actions">
            <button class="btn btn-outline" data-go="/discover">${icon('compass')} Browse mods</button>
            <button class="btn btn-primary" data-new>${icon('plus')} New instance</button>
          </div>
        </header>

        <section class="orbit-hero">
          <div class="orbit-art" aria-hidden="true">
            <span class="orbit-corona"></span>
            <span class="orbit-moon"></span>
            <span class="orbit-ring"><span class="orbit-dot"></span></span>
          </div>
          <div class="orbit-hero-body">
            <p class="eyebrow">Orbit Client</p>
            <h2>Play <span>Orbit Client</span></h2>
            <p class="orbit-hero-desc">The Orbit mod menu, HUD, cosmetics and cloth capes on Fabric 1.21.11. It updates itself every time you play.</p>
            <div class="row-wrap" style="gap:10px;margin-top:20px">
              <div class="hero-launch ${orbitRunning ? 'is-running' : ''}" style="margin-top:0">
                ${orbitRunning
                  ? html`<button class="hero-launch-main" data-stop="${orbit.id}">${icon('square')}<span>STOP<small>Orbit Client is running</small></span></button>`
                  : html`<button class="hero-launch-main" data-orbit-play ${orbitBusy ? 'disabled' : ''}>
                      ${orbitBusy ? spinner() : icon('play')}
                      <span>${store.state.orbitInstalling ? 'INSTALLING…' : orbitBusy ? 'STARTING…' : orbit ? 'PLAY' : 'INSTALL & PLAY'}<small>${orbit ? `Fabric ${orbit.gameVersion} · ${orbit.modCount} mods` : 'Sets up Fabric, Fabric API and Orbit Client'}</small></span>
                    </button>`}
              </div>
              ${server && !orbitRunning && html`<button class="btn btn-outline orbit-join" data-orbit-join ${orbitBusy ? 'disabled' : ''}>${icon('server', 'icon-sm')} Join ${server}</button>`}
              ${orbit && html`<button class="btn btn-ghost orbit-join" data-go="/instance/${orbit.id}">Manage</button>`}
            </div>
          </div>
        </section>

        ${server &&
        html`<section class="arcade-hero">
          <div class="arcade-hero-body">
            <p class="eyebrow">Insert coin to continue</p>
            <h2>Play on <span>MineArcade</span></h2>
            <p class="arcade-hero-addr">${icon('server', 'icon-sm')} ${server}</p>
            <p class="arcade-hero-desc">Jump straight onto the server with your instance.<br>Pick a different one with the arrow.</p>
            <div class="hero-launch ${heroRunning ? 'is-running' : ''}">
              ${hero
                ? heroRunning
                  ? html`<button class="hero-launch-main" data-stop="${hero.id}">${icon('square')}<span>STOP<small>${hero.name} is running</small></span></button>`
                  : html`<button class="hero-launch-main" data-join="${hero.id}" ${heroLaunching ? 'disabled' : ''}>
                      ${heroLaunching ? spinner() : icon('play')}
                      <span>${heroLaunching ? 'STARTING…' : 'JOIN SERVER'}<small>${hero.name} · ${loaderLabel(hero.loader)} ${hero.gameVersion}</small></span>
                    </button>`
                : html`<button class="hero-launch-main" data-setup>${icon('sparkles')}<span>SET UP &amp; PLAY<small>Creates an instance for you</small></span></button>`}
              ${hero && instances.length > 1 && !heroRunning &&
              html`<button class="hero-launch-pick" data-pick-hero aria-label="Choose which instance joins" data-tip="Choose instance">${icon('chevron-down')}</button>`}
            </div>
          </div>
        </section>`}

        ${!guideDismissed &&
        html`<section class="guide">
          <div class="row-between">
            <div class="row">
              <span class="metric-icon" style="width:34px;height:34px">${icon('sparkles', 'icon-sm')}</span>
              <div><div class="bold">New to Orbit Launcher?</div><div class="small muted">Three quick steps to get set up.</div></div>
            </div>
            <button class="btn btn-ghost btn-icon btn-sm" data-dismiss-guide aria-label="Dismiss">${icon('x')}</button>
          </div>
          <div class="guide-steps">
            <div class="guide-step"><span class="guide-num">01</span><div class="grow"><b>Create a setup</b><small>Choose a version and loader.</small></div><button class="btn btn-ghost btn-sm" data-new>Create ${icon('arrow-right', 'icon-sm')}</button></div>
            <div class="guide-step"><span class="guide-num">02</span><div class="grow"><b>Add something new</b><small>Mods, shaders and modpacks.</small></div><button class="btn btn-ghost btn-sm" data-go="/discover">Browse ${icon('arrow-right', 'icon-sm')}</button></div>
            <div class="guide-step"><span class="guide-num">03</span><div class="grow"><b>Jump in</b><small>${spotlight ? `Launch ${spotlight.name}.` : 'Your first instance shows up here.'}</small></div><button class="btn btn-ghost btn-sm" ${spotlight ? html`data-play="${spotlight.id}"` : html`data-go="/library"`}>Play ${icon('arrow-right', 'icon-sm')}</button></div>
          </div>
        </section>`}

        ${spotlight
          ? html`<section class="home-split">
              <div class="spotlight card">
                <div class="row" style="align-items:flex-start;gap:16px">
                  ${instanceIcon(spotlight, 68)}
                  <div class="grow">
                    <p class="eyebrow" style="color:var(--primary)">Continue playing</p>
                    <h2 class="truncate" style="font-size:24px;margin-top:6px">${spotlight.name}</h2>
                    <div class="row-wrap" style="margin-top:10px;gap:6px">
                      <span class="badge badge-mono">${spotlight.gameVersion}</span>
                      ${loaderBadge(spotlight.loader)}
                      <span class="small muted">${spotlight.modCount} ${spotlight.modCount === 1 ? 'mod' : 'mods'}</span>
                    </div>
                  </div>
                </div>
                <div class="row-between spotlight-foot">
                  <span class="small muted row" style="gap:6px">${icon('clock', 'icon-sm')} Last played ${relativeTime(spotlight.lastPlayed)}
                    ${spotlight.playTimeSecs > 0 && html` · ${formatDuration(spotlight.playTimeSecs)} total`}</span>
                  <div class="row">
                    <button class="btn btn-outline" data-go="/instance/${spotlight.id}">Manage</button>
                    ${running
                      ? html`<button class="btn btn-danger" data-stop="${spotlight.id}">${icon('square')} Stop</button>`
                      : html`<button class="btn btn-play" data-play="${spotlight.id}" ${launching ? 'disabled' : ''}>${launching ? spinner() : icon('play')} Play</button>`}
                  </div>
                </div>
              </div>
              <div class="home-stats">
                <div class="metric"><span class="metric-icon">${icon('layers')}</span><span><small>Instances</small><strong>${instances.length}</strong></span></div>
                <div class="metric"><span class="metric-icon">${icon('package')}</span><span><small>Mods installed</small><strong>${modCount}</strong></span></div>
                <div class="metric ${runningCount ? 'is-live' : ''}"><span class="metric-icon">${icon('gamepad')}</span><span><small>Playing now</small><strong>${runningCount}</strong></span></div>
              </div>
            </section>`
          : store.state.instancesLoaded &&
            emptyState({
              iconName: 'sparkles',
              title: 'Make Minecraft yours',
              description:
                'Create an instance for vanilla play, install a modpack, or bring your setup over from another launcher.',
              actions: html`
                <button class="btn btn-primary" data-new>${icon('plus')} Create instance</button>
                <button class="btn btn-outline" data-go="/discover/modpack">${icon('compass')} Explore modpacks</button>
                <button class="btn btn-outline" data-go="/library?import=launcher">${icon('folder-input')} Import</button>`,
            })}

        ${instances.length > 0 &&
        html`<section class="stack">
          <div class="row-between">
            <div><p class="eyebrow" style="color:var(--muted)">Your collection</p><h2 style="font-size:19px;margin-top:4px">Recent instances</h2></div>
            <button class="btn btn-ghost btn-sm" data-go="/library">Open library ${icon('arrow-right', 'icon-sm')}</button>
          </div>
          <div class="card-grid" data-list>
            ${[...instances]
              .sort((a, b) => new Date(b.lastPlayed ?? 0) - new Date(a.lastPlayed ?? 0))
              .slice(0, 4)
              .map((instance) => instanceCard(instance))}
          </div>
        </section>`}`,
    );
  };

  root.addEventListener('click', (event) => {
    const target = event.target;
    const go = target.closest('[data-go]');
    if (go) return navigate(go.dataset.go);
    if (target.closest('[data-new]')) return openNewInstanceDialog();
    if (target.closest('[data-dismiss-guide]')) {
      prefs.set('home.guideDismissed', true);
      return draw();
    }
    const pick = target.closest('[data-pick-hero]');
    if (pick) {
      const current = heroInstance();
      return showMenu(
        pick,
        [
          { header: 'Join with' },
          ...[...store.state.instances]
            .sort((a, b) => new Date(b.lastPlayed ?? 0) - new Date(a.lastPlayed ?? 0))
            .map((instance) => ({
              label: `${instance.name} · ${loaderLabel(instance.loader)} ${instance.gameVersion}`,
              leading: String(instanceIcon(instance, 22)),
              trail: instance.id === current?.id ? 'Selected' : '',
              onSelect: () => {
                prefs.set('home.heroInstance', instance.id);
                draw();
              },
            })),
        ],
        { align: 'end' },
      );
    }
    if (target.closest('[data-orbit-play]')) return playOrbitClient();
    if (target.closest('[data-orbit-join]')) return playOrbitClient({ type: 'multiplayer', address: featuredServer() });
    const join = target.closest('[data-join]');
    if (join) return joinFeaturedServer(join.dataset.join);
    const play = target.closest('[data-play]');
    if (play) return launchInstance(play.dataset.play);
    const stop = target.closest('[data-stop]');
    if (stop) {
      const instance = store.state.instances.find((i) => i.id === stop.dataset.stop);
      return stopInstance(stop.dataset.stop, instance?.name);
    }
    if (target.closest('[data-setup]')) {
      // No instance yet: create one, then go straight to the server.
      return openNewInstanceDialog({
        name: 'MineArcade',
        onCreated: (instance) => {
          navigate('/');
          joinFeaturedServer(instance.id);
        },
      });
    }
  });

  // Instance cards (anything inside [data-instance]); the hero and spotlight
  // buttons above carry no data-instance, so the two handlers never overlap.
  wireInstanceList(root);

  draw();
  return store.on(['instances', 'running', 'launching', 'settings', 'instancesLoaded', 'orbitInstalling'], draw);
}
