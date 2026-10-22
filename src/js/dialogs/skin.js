// Skin dialog: the signed-in account's skin as a rotating 3D model, the arm
// model, and uploading a new skin PNG. A file picked from disk previews as a
// head crop and isn't sent to Mojang until Upload is pressed.
import * as api from '../api.js';
import { refreshAccounts, signIn } from '../actions.js';
import { activeAccount } from '../store.js';
import { errorText, fileSrc, pickFile } from '../tauri.js';
import { emptyState, html, icon, openModal, setHtml, spinner, toast } from '../ui.js';

// ---------- skinview3d (vendored UMD bundle, loaded once on demand) ----------

let skinviewLoading = null;

function loadSkinview() {
  if (window.skinview3d) return Promise.resolve(window.skinview3d);
  skinviewLoading ??= new Promise((resolve, reject) => {
    const script = document.createElement('script');
    script.src = 'vendor/skinview3d.bundle.js';
    script.async = true;
    script.onload = () =>
      window.skinview3d ? resolve(window.skinview3d) : reject(new Error('skinview3d did not load.'));
    script.onerror = () => {
      script.remove();
      skinviewLoading = null;
      reject(new Error("The 3D skin viewer couldn't be loaded."));
    };
    document.head.appendChild(script);
  });
  return skinviewLoading;
}

// ---------- Head crop (port of Aether's lib/skin.ts) ----------

// Skin sheets are 64px wide; the head's front face is the 8x8 block at (8, 8)
// and its outer "hat" layer the one at (40, 8). Legacy 64x32 sheets have no
// hat layer: their right half mirrors the left.
const SHEET_WIDTH = 64;
const HEAD_SIZE = 8;
const FACE = { x: 8, y: 8 };
const HAT = { x: 40, y: 8 };
const LEGACY_HEIGHT = 32;

/** `background-position` for one axis, as a percentage of (box - sheet). */
const axisOffset = (sourcePx, sheetPx, boxes) => (100 * sourcePx * boxes) / (sheetPx * (boxes - 1));

/**
 * Inline style that crops a skin sheet down to its head (face plus hat layer),
 * in percentages so it works at any size. `sheetHeight` tells modern and
 * legacy sheets apart; pass null while it is unknown to assume the modern one.
 */
export function headCrop(sheetUrl, sheetHeight) {
  const height = sheetHeight !== null && sheetHeight >= LEGACY_HEIGHT ? sheetHeight : SHEET_WIDTH;
  const widthBoxes = SHEET_WIDTH / HEAD_SIZE;
  const heightBoxes = height / HEAD_SIZE;
  const size = `${widthBoxes * 100}% ${heightBoxes * 100}%`;
  const url = `url("${String(sheetUrl).replace(/["\\]/g, '\\$&')}")`;
  const face = `${axisOffset(FACE.x, SHEET_WIDTH, widthBoxes)}% ${axisOffset(FACE.y, height, heightBoxes)}%`;

  if (height <= LEGACY_HEIGHT) {
    return `background-image:${url};background-size:${size};background-position:${face};background-repeat:no-repeat`;
  }
  // The first layer paints on top, so the hat goes first, as in game.
  const hat = `${axisOffset(HAT.x, SHEET_WIDTH, widthBoxes)}% ${axisOffset(HAT.y, height, heightBoxes)}%`;
  return `background-image:${url},${url};background-size:${size},${size};background-position:${hat},${face};background-repeat:no-repeat,no-repeat`;
}

/** A head-crop element; re-crops once the sheet's real height is known. */
function headCropNode(container, url) {
  const node = container.querySelector('[data-head]');
  if (!node) return;
  node.setAttribute('style', headCrop(url, null));
  const probe = new Image();
  probe.onload = () => node.isConnected && node.setAttribute('style', headCrop(url, probe.naturalHeight));
  probe.src = url;
}

// ---------- Dialog ----------

export function openSkinDialog() {
  let data = null;
  let loading = true;
  let error = null;
  let pickedPath = null;
  let variant = 'classic';
  let uploading = false;
  let viewer = null;
  let viewerUrl = null;
  let viewerFailed = false;
  let stageToken = 0;

  const signedIn = Boolean(activeAccount());

  const modal = openModal({
    title: 'Skin',
    description: signedIn
      ? `How ${activeAccount().username} looks in game, and what to wear next.`
      : 'Sign in to see and change your skin.',
    size: 'lg',
    body: html`<div data-skin></div>`,
    footer: html`<button class="btn btn-ghost" data-close>Close</button>`,
    onClose: disposeViewer,
  });
  const slot = modal.body.querySelector('[data-skin]');

  function disposeViewer() {
    stageToken++;
    try {
      viewer?.dispose();
    } catch (cause) {
      console.error('skin viewer dispose failed', cause);
    }
    viewer = null;
    viewerUrl = null;
  }

  if (!signedIn) {
    setHtml(
      slot,
      emptyState({
        iconName: 'shirt',
        title: 'Nobody is signed in',
        description: 'Sign in with the Microsoft account that owns Minecraft to see your skin and change it.',
        actions: html`<button class="btn btn-primary" data-sign-in>${icon('user')} Sign in with Microsoft</button>`,
      }),
    );
    slot.addEventListener('click', (event) => {
      if (!event.target.closest('[data-sign-in]')) return;
      modal.close();
      signIn();
    });
    return;
  }

  // ----- rendering -----

  function draw() {
    if (modal.closed) return;
    if (loading) {
      setHtml(slot, html`<div class="skeleton" style="height:360px"></div>`);
      return;
    }
    const account = data?.account ?? null;
    setHtml(
      slot,
      html`
        ${error && html`<p class="form-error selectable">${error}</p>`}
        <div class="skin-layout">
          <div class="skin-stage" data-stage></div>
          <div class="stack-lg">
            <div class="stack-sm">
              <span class="field-label">Arm model</span>
              <span class="hint">Slim arms are 3 pixels wide, classic arms 4.</span>
              <div class="segmented" style="align-self:flex-start">
                ${['classic', 'slim'].map(
                  (option) => html`<button data-variant="${option}" aria-pressed="${variant === option}" ${!account || uploading ? 'disabled' : ''}>
                    ${option === 'classic' ? 'Classic' : 'Slim'}
                  </button>`,
                )}
              </div>
            </div>
            <hr class="divider">
            <div class="stack-sm">
              <span class="field-label">Upload a new skin</span>
              <span class="hint">A PNG sheet, 64×64 (or the legacy 64×32). Nothing is uploaded until you press Upload.</span>
              <div class="row-wrap" style="margin-top:4px">
                <button class="btn btn-outline btn-sm" data-choose ${!account || uploading ? 'disabled' : ''}>
                  ${icon('image', 'icon-sm')} ${pickedPath ? 'Choose another' : 'Choose file'}
                </button>
                ${pickedPath && html`<button class="btn btn-ghost btn-sm" data-cancel-pick ${uploading ? 'disabled' : ''}>${icon('x', 'icon-sm')} Cancel</button>`}
                <button class="btn btn-primary btn-sm" data-upload ${!pickedPath || uploading ? 'disabled' : ''}>
                  ${uploading ? spinner() : icon('upload', 'icon-sm')} ${uploading ? 'Uploading…' : 'Upload'}
                </button>
              </div>
              ${pickedPath && html`<span class="small muted truncate selectable" title="${pickedPath}">${pickedPath}</span>`}
            </div>
          </div>
        </div>`,
    );
    drawStage();
  }

  function drawStage() {
    const stage = slot.querySelector('[data-stage]');
    if (!stage) return;
    const remoteUrl = data?.skin?.skinUrl ?? null;

    // A file straight off disk can't be uploaded to WebGL without cross-origin
    // access from the asset protocol, so it previews as a head crop.
    if (pickedPath) {
      disposeViewer();
      const url = fileSrc(pickedPath);
      setHtml(
        stage,
        html`<div class="skin-head-preview">
          <div class="skin-head" data-head role="img" aria-label="Selected skin preview"></div>
          <p class="small muted">Preview of the file you picked. Press Upload to wear it.</p>
        </div>`,
      );
      headCropNode(stage, url);
      return;
    }

    if (!remoteUrl) {
      disposeViewer();
      setHtml(stage, html`<p class="small muted center" style="padding:0 24px">No skin on this account yet.</p>`);
      return;
    }

    if (viewerFailed) {
      disposeViewer();
      setHtml(
        stage,
        html`<div class="skin-head-preview">
          <div class="skin-head" data-head role="img" aria-label="Skin preview"></div>
          <p class="small muted">The 3D preview is unavailable right now. The skin itself is fine.</p>
        </div>`,
      );
      headCropNode(stage, remoteUrl);
      return;
    }

    // setHtml replaced the stage, so the old canvas is gone: start afresh.
    disposeViewer();
    setHtml(stage, html`<canvas class="skin-canvas" width="258" height="358"></canvas><span class="skin-stage-loading">${spinner()}</span>`);
    mountViewer(stage, remoteUrl);
  }

  async function mountViewer(stage, url) {
    const token = ++stageToken;
    const canvas = stage.querySelector('canvas');
    try {
      const { SkinViewer, IdleAnimation } = await loadSkinview();
      // The dialog may have closed (or the stage changed) while loading.
      if (token !== stageToken || modal.closed || !canvas.isConnected) return;
      viewer = new SkinViewer({
        canvas,
        width: 258,
        height: 358,
        zoom: 0.9,
        model: variant === 'slim' ? 'slim' : 'default',
      });
      viewer.autoRotate = true;
      viewer.animation = new IdleAnimation();
      viewerUrl = url;
      await viewer.loadSkin(url, { model: variant === 'slim' ? 'slim' : 'default' });
      if (data?.skin?.capeUrl) await viewer.loadCape(data.skin.capeUrl).catch(() => {});
      if (token !== stageToken) return;
      stage.querySelector('.skin-stage-loading')?.remove();
    } catch (cause) {
      console.error('skin viewer failed', cause);
      if (token !== stageToken || modal.closed) return;
      viewerFailed = true;
      drawStage();
    }
  }

  // ----- actions -----

  async function chooseFile() {
    try {
      const path = await pickFile({ title: 'Choose a skin', extensions: ['png'] });
      if (path && !modal.closed) {
        pickedPath = path;
        draw();
      }
    } catch (cause) {
      toast(errorText(cause), { kind: 'error' });
    }
  }

  async function upload() {
    if (!pickedPath || uploading) return;
    uploading = true;
    modal.setDismissable(false);
    draw();
    try {
      const result = await api.skin.upload(pickedPath, variant);
      data = result;
      variant = result?.skin?.variant ?? variant;
      pickedPath = null;
      viewerFailed = false;
      error = null;
      // The sidebar avatar reads from the account list, so reload it too.
      await refreshAccounts();
      toast('Skin updated', { kind: 'success' });
    } catch (cause) {
      toast("The skin couldn't be uploaded", { kind: 'error', description: errorText(cause) });
    } finally {
      uploading = false;
      modal.setDismissable(true);
    }
    draw();
  }

  slot.addEventListener('click', (event) => {
    const target = event.target;
    const option = target.closest('[data-variant]');
    if (option) {
      if (option.dataset.variant === variant) return;
      variant = option.dataset.variant;
      slot.querySelectorAll('[data-variant]').forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.variant === variant)));
      // Reload the texture on the existing model instead of rebuilding it.
      if (viewer && viewerUrl) {
        viewer.loadSkin(viewerUrl, { model: variant === 'slim' ? 'slim' : 'default' })?.catch?.(() => {});
      }
      return;
    }
    if (target.closest('[data-choose]')) return chooseFile();
    if (target.closest('[data-cancel-pick]')) {
      pickedPath = null;
      return draw();
    }
    if (target.closest('[data-upload]')) return upload();
  });

  // A fresh read every time: the skin may have changed in game or on minecraft.net.
  draw();
  api.skin
    .current()
    .then((result) => {
      data = result;
      variant = result?.skin?.variant ?? 'classic';
    })
    .catch((cause) => {
      error = errorText(cause);
    })
    .finally(() => {
      loading = false;
      if (modal.closed) return;
      if (!data?.account && !error) {
        setHtml(
          slot,
          emptyState({
            iconName: 'shirt',
            title: 'Nobody is signed in',
            description: 'Sign in with the Microsoft account that owns Minecraft to see your skin and change it.',
          }),
        );
        return;
      }
      draw();
    });
}
