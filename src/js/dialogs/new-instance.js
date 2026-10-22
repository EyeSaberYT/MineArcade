// "New instance" dialog: name, icon, Minecraft version, loader + loader version.
import * as api from '../api.js';
import { refreshInstances } from '../actions.js';
import { navigate } from '../router.js';
import { store } from '../store.js';
import { pickIcon } from './icon-picker.js';
import {
  html,
  icon,
  instanceIcon,
  LOADERS,
  loaderLabel,
  openModal,
  setHtml,
  spinner,
  toast,
} from '../ui.js';

/**
 * Open the dialog. options.name pre-fills the name; options.onCreated(instance)
 * runs instead of navigating to the new instance.
 */
export function openNewInstanceDialog({ name = '', onCreated } = {}) {
  let iconValue = null;
  let loader = 'vanilla';
  let versionList = [];
  let loaderRequest = 0;

  const modal = openModal({
    title: 'New instance',
    description: 'Pick a Minecraft version and, optionally, a mod loader.',
    body: html`
      <label class="field">
        <span>Name</span>
        <input class="input" data-name maxlength="64" value="${name}" placeholder="Leave empty to name it after the version" autofocus>
      </label>
      <div class="field">
        <span>Icon</span>
        <div class="row" data-icon-row></div>
      </div>
      <div class="grid-2">
        <label class="field">
          <span>Minecraft version</span>
          <select class="input" data-version disabled><option>Loading…</option></select>
        </label>
        <label class="field">
          <span>Mod loader</span>
          <select class="input" data-loader>
            ${LOADERS.map((kind) => html`<option value="${kind}">${loaderLabel(kind)}</option>`)}
          </select>
        </label>
      </div>
      <label class="check-row"><input type="checkbox" data-snapshots ${store.state.settings?.showSnapshots ? 'checked' : ''}> Show snapshots and old versions</label>
      <div data-loader-fields hidden class="stack">
        <label class="field">
          <span>Loader version</span>
          <select class="input" data-loader-version disabled></select>
        </label>
        <label class="switch-row">
          <input type="checkbox" data-install checked>
          <span class="switch"></span>
          <span>Install the loader now <span class="hint" style="display:block">Downloads the loader profile right away instead of at first launch.</span></span>
        </label>
      </div>
      <p class="form-error" data-error></p>`,
    footer: html`
      <button class="btn btn-ghost" data-close>Cancel</button>
      <button class="btn btn-primary" data-create disabled>${icon('package')} Create instance</button>`,
  });

  const body = modal.body;
  const nameInput = body.querySelector('[data-name]');
  const versionSelect = body.querySelector('[data-version]');
  const loaderSelect = body.querySelector('[data-loader]');
  const loaderVersionSelect = body.querySelector('[data-loader-version]');
  const loaderFields = body.querySelector('[data-loader-fields]');
  const snapshots = body.querySelector('[data-snapshots]');
  const errorEl = body.querySelector('[data-error]');
  const createBtn = modal.foot.querySelector('[data-create]');

  const renderIcon = () => {
    setHtml(
      body.querySelector('[data-icon-row]'),
      html`${instanceIcon({ icon: iconValue, name: nameInput.value || 'New instance', id: nameInput.value || 'new' }, 52)}
        <button class="btn btn-outline btn-sm" data-pick-icon>${icon('image')} Change icon</button>`,
    );
  };
  body.addEventListener('click', async (event) => {
    if (!event.target.closest('[data-pick-icon]')) return;
    const chosen = await pickIcon(event.target.closest('[data-pick-icon]'), iconValue);
    if (chosen !== undefined) {
      iconValue = chosen;
      renderIcon();
    }
  });
  nameInput.addEventListener('input', renderIcon);
  renderIcon();

  const updateCreate = () => {
    const needsLoaderVersion = loader !== 'vanilla' && !loaderVersionSelect.value;
    createBtn.disabled = !versionSelect.value || versionSelect.disabled || needsLoaderVersion;
  };

  async function loadVersions() {
    versionSelect.disabled = true;
    createBtn.disabled = true;
    try {
      versionList = await api.versions.list(snapshots.checked);
      if (modal.closed) return;
      const previous = versionSelect.value;
      setHtml(
        versionSelect,
        versionList.map(
          (v) =>
            html`<option value="${v.id}">${v.id}${v.latestRelease ? ' · latest' : ''}${v.versionType !== 'release' ? ` (${v.versionType.replace('_', ' ')})` : ''}</option>`,
        ),
      );
      if (versionList.some((v) => v.id === previous)) versionSelect.value = previous;
      versionSelect.disabled = false;
      await loadLoaderVersions();
    } catch (error) {
      errorEl.textContent = `Couldn't load the version list: ${error}`;
    }
    updateCreate();
  }

  async function loadLoaderVersions() {
    loaderFields.hidden = loader === 'vanilla';
    if (loader === 'vanilla' || !versionSelect.value) {
      updateCreate();
      return;
    }
    const request = ++loaderRequest;
    loaderVersionSelect.disabled = true;
    setHtml(loaderVersionSelect, html`<option value="">Loading…</option>`);
    updateCreate();
    try {
      const list = await api.versions.loaderVersions(loader, versionSelect.value);
      if (request !== loaderRequest || modal.closed) return;
      if (!list.length) {
        setHtml(loaderVersionSelect, html`<option value="">${loaderLabel(loader)} doesn't support ${versionSelect.value}</option>`);
      } else {
        setHtml(
          loaderVersionSelect,
          list.map((entry) => html`<option value="${entry.version}">${entry.version}${entry.stable ? ' · stable' : ''}</option>`),
        );
        loaderVersionSelect.value = (list.find((entry) => entry.stable) ?? list[0]).version;
        loaderVersionSelect.disabled = false;
      }
    } catch (error) {
      if (request === loaderRequest) {
        setHtml(loaderVersionSelect, html`<option value="">Couldn't load ${loaderLabel(loader)} versions</option>`);
        errorEl.textContent = String(error);
      }
    }
    updateCreate();
  }

  snapshots.addEventListener('change', loadVersions);
  versionSelect.addEventListener('change', loadLoaderVersions);
  loaderSelect.addEventListener('change', () => {
    loader = loaderSelect.value;
    errorEl.textContent = '';
    loadLoaderVersions();
  });
  loaderVersionSelect.addEventListener('change', updateCreate);

  createBtn.addEventListener('click', async () => {
    const gameVersion = versionSelect.value;
    const finalName =
      nameInput.value.trim() || (loader === 'vanilla' ? `Minecraft ${gameVersion}` : `${loaderLabel(loader)} ${gameVersion}`);
    errorEl.textContent = '';
    createBtn.disabled = true;
    createBtn.innerHTML = `${spinner()} Creating…`;
    modal.setDismissable(false);
    try {
      const instance = await api.instances.create({
        name: finalName,
        gameVersion,
        loader,
        icon: iconValue,
        loaderVersion: loader === 'vanilla' ? null : loaderVersionSelect.value || null,
        installLoader: loader !== 'vanilla' && body.querySelector('[data-install]').checked,
      });
      await refreshInstances();
      modal.setDismissable(true);
      modal.close();
      toast(`Created “${instance.name}”`, { kind: 'success' });
      if (onCreated) onCreated(instance);
      else navigate(`/instance/${instance.id}`);
    } catch (error) {
      modal.setDismissable(true);
      errorEl.textContent = String(error);
      createBtn.innerHTML = `${icon('package')} Create instance`;
      updateCreate();
    }
  });

  loadVersions();
}
