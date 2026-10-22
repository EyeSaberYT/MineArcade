// Library: every instance, with search, grid/list views and import entry points.
import { instanceCard, instanceRow, wireInstanceList } from '../components/instance-card.js';
import { openNewInstanceDialog } from '../dialogs/new-instance.js';
import { setDropHandler } from '../drop.js';
import { prefs, store } from '../store.js';
import { emptyState, html, icon, setHtml, showMenu } from '../ui.js';

const openLauncherImport = async () => (await import('../dialogs/import-launcher.js')).openImportLauncherDialog();
const openModpackImport = async (path) => (await import('../dialogs/import-modpack.js')).openImportModpackDialog(path);
const openScan = async (instance) => (await import('../dialogs/scan.js')).openScanDialog(instance.id, instance.name);

export function render(root, _params, query) {
  let filter = '';
  let view = prefs.get('library.view', 'grid');

  setHtml(
    root,
    html`
      <header class="page-head">
        <div>
          <h1>Instances</h1>
          <p>Every Minecraft setup, organised and ready to launch.</p>
        </div>
        <div class="page-actions">
          <div class="segmented" data-view>
            <button data-mode="grid" title="Grid">${icon('grid', 'icon-sm')}</button>
            <button data-mode="list" title="List">${icon('list', 'icon-sm')}</button>
          </div>
          <button class="btn btn-outline" data-import>${icon('download')} Import ${icon('chevron-down', 'icon-sm')}</button>
          <button class="btn btn-primary" data-new>${icon('plus')} New instance</button>
        </div>
      </header>
      <section class="grid-3" data-metrics></section>
      <section class="card" style="overflow:hidden">
        <div class="row-between" style="padding:12px;border-bottom:1px solid var(--line)">
          <label class="input-icon grow" style="max-width:420px">
            ${icon('search', 'icon-sm')}
            <input class="input" data-filter placeholder="Find an instance by name, version or loader">
          </label>
          <span class="small muted" data-count></span>
        </div>
        <div style="padding:16px" data-list></div>
      </section>`,
  );

  const listEl = root.querySelector('[data-list]');
  const metricsEl = root.querySelector('[data-metrics]');
  const countEl = root.querySelector('[data-count]');

  const draw = () => {
    const instances = store.state.instances;
    const runningCount = instances.filter((i) => store.state.running.has(i.id)).length;
    const modCount = instances.reduce((sum, i) => sum + i.modCount, 0);
    setHtml(
      metricsEl,
      html`
        <div class="metric"><span class="metric-icon">${icon('layers')}</span><span><small>My instances</small><strong>${instances.length}</strong></span></div>
        <div class="metric"><span class="metric-icon">${icon('package')}</span><span><small>Mods installed</small><strong>${modCount}</strong></span></div>
        <div class="metric ${runningCount ? 'is-live' : ''}"><span class="metric-icon">${icon('gamepad')}</span><span><small>Currently running</small><strong>${runningCount}</strong></span></div>`,
    );

    root.querySelectorAll('[data-mode]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.mode === view)));

    const needle = filter.trim().toLowerCase();
    const shown = needle
      ? instances.filter((i) => [i.name, i.gameVersion, i.loader].some((f) => f.toLowerCase().includes(needle)))
      : instances;
    countEl.textContent = needle
      ? `${shown.length} results`
      : `${instances.length} ${instances.length === 1 ? 'instance' : 'instances'}`;

    if (!store.state.instancesLoaded) {
      setHtml(listEl, html`<div class="card-grid">${Array.from({ length: 6 }, () => html`<div class="skeleton" style="height:170px"></div>`)}</div>`);
    } else if (!shown.length) {
      setHtml(
        listEl,
        emptyState({
          iconName: 'layers',
          title: needle ? 'No matching instances' : 'Your library is empty',
          description: needle
            ? 'Try another name, game version or loader.'
            : 'Create an instance, import one from another launcher, or start with a modpack. You can also drop a .mrpack file here.',
          actions: needle
            ? ''
            : html`<button class="btn btn-primary" data-new>${icon('plus')} New instance</button>
                <button class="btn btn-outline" data-import-pack>${icon('package')} Import modpack</button>
                <button class="btn btn-outline" data-import-launcher>${icon('folder-input')} From another launcher</button>`,
        }),
      );
    } else if (view === 'list') {
      setHtml(listEl, html`<div class="list">${shown.map((i) => instanceRow(i, { scan: true }))}</div>`);
    } else {
      setHtml(listEl, html`<div class="card-grid">${shown.map((i) => instanceCard(i, { scan: true }))}</div>`);
    }
  };

  wireInstanceList(listEl, { onScan: openScan });

  root.addEventListener('click', (event) => {
    const target = event.target;
    const mode = target.closest('[data-mode]');
    if (mode) {
      view = mode.dataset.mode;
      prefs.set('library.view', view);
      return draw();
    }
    if (target.closest('[data-new]')) return openNewInstanceDialog();
    if (target.closest('[data-import-pack]')) return openModpackImport();
    if (target.closest('[data-import-launcher]')) return openLauncherImport();
    const importBtn = target.closest('[data-import]');
    if (importBtn) {
      showMenu(
        importBtn,
        [
          { label: 'From another launcher', icon: 'folder-input', onSelect: openLauncherImport },
          { label: 'Modpack file (.mrpack / .zip)', icon: 'package', onSelect: () => openModpackImport() },
        ],
        { align: 'end' },
      );
    }
  });

  root.querySelector('[data-filter]').addEventListener('input', (event) => {
    filter = event.target.value;
    draw();
  });

  // Dropping a modpack anywhere on this page imports it.
  const offDrop = setDropHandler({
    label: 'Drop a .mrpack or CurseForge .zip to import it',
    accepts: (paths) => paths.some((p) => /\.(mrpack|zip)$/i.test(p)),
    onDrop: (paths) => openModpackImport(paths.find((p) => /\.(mrpack|zip)$/i.test(p))),
  });

  if (query.get('import') === 'launcher') openLauncherImport();
  if (query.get('import') === 'modpack') openModpackImport();

  draw();
  const off = store.on(['instances', 'running', 'launching', 'instancesLoaded', 'settings'], draw);
  return () => {
    off();
    offDrop();
  };
}
