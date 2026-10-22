// Home: the MineArcade server hero, "continue playing", stats and recent instances.
import {
  featuredServer,
  joinFeaturedServer,
  launchInstance,
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
  relativeTime,
  setHtml,
  spinner,
} from '../ui.js';

export function render(root) {
  const draw = () => {
    const instances = store.state.instances;
    const spotlight = spotlightInstance();
    const server = featuredServer();
    const running = spotlight && store.state.running.has(spotlight.id);
    const launching = spotlight && store.state.launching.has(spotlight.id);
    const runningCount = instances.filter((i) => store.state.running.has(i.id)).length;
    const modCount = instances.reduce((sum, i) => sum + i.modCount, 0);
    const guideDismissed = prefs.get('home.guideDismissed', false);

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

        ${server &&
        html`<section class="arcade-hero">
          <div class="arcade-hero-pixels" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i></div>
          <div class="arcade-hero-body">
            <p class="eyebrow">Insert coin to continue</p>
            <h2>Play on <span>MineArcade</span></h2>
            <p class="arcade-hero-addr">${icon('server', 'icon-sm')} ${server}</p>
            <div class="row-wrap" style="margin-top:20px">
              ${spotlight
                ? running
                  ? html`<button class="btn btn-play btn-lg is-running" data-stop="${spotlight.id}">${icon('square')} STOP</button>`
                  : html`<button class="btn btn-play btn-lg" data-join="${spotlight.id}" ${launching ? 'disabled' : ''}>
                      ${launching ? spinner() : icon('play')} ${launching ? 'STARTING…' : 'JOIN SERVER'}
                    </button>`
                : html`<button class="btn btn-play btn-lg" data-setup>${icon('sparkles')} SET UP & PLAY</button>`}
              ${spotlight &&
              html`<span class="small muted">with <a data-go="/instance/${spotlight.id}">${spotlight.name}</a> · ${spotlight.gameVersion}</span>`}
            </div>
          </div>
        </section>`}

        ${!guideDismissed &&
        html`<section class="guide">
          <div class="row-between">
            <div class="row">
              <span class="metric-icon" style="width:34px;height:34px">${icon('sparkles', 'icon-sm')}</span>
              <div><div class="bold">New to MineArcade?</div><div class="small muted">Three quick steps to get set up.</div></div>
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
  return store.on(['instances', 'running', 'launching', 'settings', 'instancesLoaded'], draw);
}
