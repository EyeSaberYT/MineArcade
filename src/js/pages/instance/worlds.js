// Worlds tab: the instance's saves and the zip backups MineArcade has taken of
// them. Backing up and restoring touch files the game writes while it runs, so
// both wait until the instance is closed; deleting a backup is always allowed.
import * as api from '../../api.js';
import { launchInstance } from '../../actions.js';
import { store } from '../../store.js';
import { fileSrc } from '../../tauri.js';
import {
  confirmDialog,
  emptyState,
  errorBlock,
  formatBytes,
  html,
  icon,
  loadingBlock,
  relativeTime,
  setHtml,
  spinner,
  toast,
  toastError,
} from '../../ui.js';
import { isLaunching, isRunning, plural, reveal } from './shared.js';

/** Backups grouped by the world they came from, alphabetically; newest first inside a group. */
function groupBackups(overview) {
  const groups = new Map();
  for (const backup of overview.backups) {
    if (!groups.has(backup.world)) groups.set(backup.world, []);
    groups.get(backup.world).push(backup);
  }
  return [...groups.entries()]
    .map(([world, backups]) => ({ world, exists: overview.worlds.some((w) => w.name === world), backups }))
    .sort((a, b) => a.world.localeCompare(b.world));
}

export function mountWorlds(pane, ctx) {
  const { id } = ctx;
  let overview = null;
  let loadError = null;
  let loading = true;
  /** What is in flight, as `<action>:<name>`, so only that row shows a spinner. */
  let busy = null;

  function worldIcon(world) {
    const fallback = html`<div class="inst-thumb">${icon('globe', 'icon-sm')}</div>`;
    if (!world.icon) return fallback;
    return html`<div class="inst-thumb"><img src="${fileSrc(world.icon)}" alt="" loading="lazy" data-fallback="${String(fallback)}"></div>`;
  }

  function draw() {
    if (loading) return setHtml(pane, loadingBlock("Looking through this instance's worlds…"));
    if (loadError || !overview) {
      setHtml(pane, errorBlock(loadError ?? 'No data', 'worlds-retry'));
      pane.querySelector('#worlds-retry')?.setAttribute('data-reload', '');
      return;
    }

    const running = isRunning(id);
    const starting = isLaunching(id);
    const groups = groupBackups(overview);
    const closeFirst = 'Close the game first';

    setHtml(
      pane,
      html`
        <div class="stack-lg">
          ${running &&
          html`<div class="inst-note inst-note-warn">
            ${icon('alert', 'icon-sm')}
            <div><strong>This instance is running</strong>
              <div>Minecraft writes to its world folders while it plays, so MineArcade can't back up or restore one until the game closes. Backups can still be deleted.</div></div>
          </div>`}

          <section class="stack-sm">
            <div class="row-between">
              <h3 class="inst-section-title">Worlds <span class="muted">${overview.worlds.length ? plural(overview.worlds.length, 'world') : 'none yet'}</span></h3>
              <div class="row" style="gap:6px">
                <button class="btn btn-ghost btn-icon btn-sm" data-reload title="Reload">${icon('refresh', 'icon-sm')}</button>
                <button class="btn btn-ghost btn-sm" data-reveal="${overview.savesPath}" ${overview.worlds.length ? '' : 'disabled'}>${icon('folder', 'icon-sm')} Open saves</button>
              </div>
            </div>
            ${overview.worlds.length === 0
              ? emptyState({
                  iconName: 'globe',
                  title: 'No worlds yet',
                  description: 'Worlds turn up here once you have played this instance and the game has created a saves folder.',
                })
              : html`<div class="list">${overview.worlds.map(
                  (world) => html`
                    <div class="list-row" data-world="${world.name}">
                      ${worldIcon(world)}
                      <div class="grow">
                        <strong class="truncate" style="display:block">${world.name}</strong>
                        <span class="tiny muted">${formatBytes(world.size)} · ${world.modified ? `played ${relativeTime(world.modified)}` : 'never played'}</span>
                      </div>
                      <div class="row-actions">
                        <button class="btn btn-play btn-sm" data-play-world ${running || starting ? 'disabled' : ''} title="${running ? 'This instance is already running' : `Launch straight into “${world.name}”`}">${icon('play', 'icon-sm')} Play</button>
                        <button class="btn btn-icon btn-sm" data-reveal="${world.path}" title="Open world folder">${icon('folder', 'icon-sm')}</button>
                        <button class="btn btn-outline btn-sm" data-backup ${running || busy ? 'disabled' : ''} title="${running ? closeFirst : `Zip “${world.name}” into a backup`}">
                          ${busy === `backup:${world.name}` ? spinner('icon-sm') : icon('archive', 'icon-sm')} Back up
                        </button>
                      </div>
                    </div>`,
                )}</div>`}
          </section>

          <section class="stack-sm">
            <div class="row-between">
              <h3 class="inst-section-title">Backups <span class="muted">${overview.backups.length ? plural(overview.backups.length, 'backup') : 'none yet'}</span></h3>
              <button class="btn btn-ghost btn-sm" data-reveal="${overview.backupsPath}" ${overview.backups.length ? '' : 'disabled'}>${icon('folder', 'icon-sm')} Open backups</button>
            </div>
            ${overview.backups.length === 0
              ? emptyState({
                  iconName: 'archive',
                  title: 'No backups yet',
                  description: 'Back up a world and it becomes a zip here, ready to put back if something goes wrong.',
                })
              : groups.map(
                  (group) => html`
                    <div class="stack-sm" style="margin-bottom:8px">
                      <div class="inst-group-head">
                        ${group.world}
                        ${!group.exists && html`<span class="badge">${icon('alert', 'icon-sm')} not in this instance</span>`}
                      </div>
                      <div class="list">${group.backups.map(
                        (backup) => html`
                          <div class="list-row" data-backup-name="${backup.name}">
                            <div class="inst-thumb">${icon('archive', 'icon-sm')}</div>
                            <div class="grow">
                              <div class="mono small truncate">${backup.name}</div>
                              <span class="tiny muted">${formatBytes(backup.size)} · ${backup.created ? relativeTime(backup.created) : 'unknown date'}</span>
                            </div>
                            <div class="row-actions">
                              <button class="btn btn-outline btn-sm" data-restore ${running || busy ? 'disabled' : ''}
                                title="${running ? closeFirst : group.exists ? `Replace “${group.world}” with this backup` : `Recreate “${group.world}” from this backup`}">
                                ${busy === `restore:${backup.name}` ? spinner('icon-sm') : icon('rotate-ccw', 'icon-sm')} Restore
                              </button>
                              <button class="btn btn-icon btn-sm danger" data-delete-backup ${busy ? 'disabled' : ''} title="Delete this backup">
                                ${busy === `delete:${backup.name}` ? spinner('icon-sm') : icon('trash', 'icon-sm')}
                              </button>
                            </div>
                          </div>`,
                      )}</div>
                    </div>`,
                )}
          </section>
        </div>`,
    );
  }

  async function load(silent = false) {
    if (!silent) {
      loading = true;
      draw();
    }
    try {
      const next = await api.worlds.overview(id);
      if (!ctx.alive()) return;
      overview = next;
      loadError = null;
    } catch (error) {
      if (!ctx.alive()) return;
      loadError = error;
    }
    loading = false;
    draw();
  }

  async function run(tag, task) {
    busy = tag;
    draw();
    try {
      await task();
    } catch (error) {
      toastError(error);
    }
    busy = null;
    if (ctx.alive()) draw();
  }

  const backUp = (world) =>
    run(`backup:${world}`, async () => {
      const next = await api.worlds.backup(id, world);
      if (ctx.alive()) overview = next;
      toast(`Backed up “${world}”`, { kind: 'success' });
    });

  async function restore(backup) {
    const exists = overview.worlds.some((w) => w.name === backup.world);
    const ok = await confirmDialog({
      title: `Restore “${backup.world}”?`,
      message: exists
        ? `The world in this instance is replaced with what ${backup.name} holds. Everything currently in it is zipped into a new backup first, so this can be undone.`
        : `“${backup.world}” is no longer in this instance, so it is recreated from ${backup.name}.`,
      confirmLabel: 'Restore',
    });
    if (!ok || !ctx.alive()) return;
    await run(`restore:${backup.name}`, async () => {
      const result = await api.worlds.restore(id, backup.name);
      toast(
        result.safety
          ? `Restored “${result.world}”. The world it replaced is saved as “${result.safety}”.`
          : `Restored “${result.world}”`,
        { kind: 'success' },
      );
      // Restoring rewrites the world folder, so sizes and dates moved.
      if (ctx.alive()) await load(true);
    });
  }

  async function deleteBackup(backup) {
    const ok = await confirmDialog({
      title: 'Delete this backup?',
      message: `${backup.name} is deleted. A zip can't be brought back, so this one is gone for good.`,
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!ok || !ctx.alive()) return;
    await run(`delete:${backup.name}`, async () => {
      const next = await api.worlds.deleteBackup(id, backup.name);
      if (ctx.alive()) overview = next;
      toast(`Deleted ${backup.name}`, { kind: 'success' });
    });
  }

  pane.addEventListener('click', (event) => {
    const t = event.target;
    if (t.closest('[data-reload]')) return load();
    const revealBtn = t.closest('[data-reveal]');
    if (revealBtn) return reveal(revealBtn.dataset.reveal);

    const world = t.closest('[data-world]')?.dataset.world;
    if (world != null) {
      if (t.closest('[data-backup]')) return backUp(world);
      if (t.closest('[data-play-world]')) return launchInstance(id, { type: 'singleplayer', world });
    }

    const backupName = t.closest('[data-backup-name]')?.dataset.backupName;
    const backup = backupName != null ? overview?.backups.find((b) => b.name === backupName) : null;
    if (!backup) return;
    if (t.closest('[data-restore]')) return restore(backup);
    if (t.closest('[data-delete-backup]')) return deleteBackup(backup);
  });

  // Running state gates backup/restore; a finished session also changes the worlds.
  let wasRunning = isRunning(id);
  const off = store.on(['running', 'launching'], () => {
    const now = isRunning(id);
    if (wasRunning && !now) load(true);
    else if (!loading) draw();
    wasRunning = now;
  });

  load();
  return off;
}
