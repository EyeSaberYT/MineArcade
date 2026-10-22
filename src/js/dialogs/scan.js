// Mod safety scan: runs `scan_instance` and shows what it found.
//
// ScanReport { files: ScanFileReport[], examined, critical, warnings, provenanceChecked, durationMs }
// ScanFileReport { file, kind, size, sha1, findings: ScanFinding[], provenance, truncated }
// ScanFinding { severity: 'info'|'warning'|'critical', code, summary, detail, entry }
// ScanProvenance { status: 'verified'|'unknown'|'unchecked', project, version, projectId }
//
// The framing matters as much as the rules: the backend reports evidence, so
// the copy says "review this", never "this is a virus". A file that simply
// isn't on Modrinth is described as exactly that.
import * as api from '../api.js';
import { errorText } from '../tauri.js';
import { formatBytes, html, icon, openModal, projectTypeLabel, setHtml, severityLabel, spinner } from '../ui.js';

const SEVERITY_RANK = { info: 0, warning: 1, critical: 2 };
const SEVERITY_ICON = { critical: 'shield', warning: 'alert', info: 'info' };
const plural = (count, word) => `${count} ${word}${count === 1 ? '' : 's'}`;
/** `mods/sodium.jar` → `sodium.jar`. */
const fileName = (path) => path.split('/').pop() || path;
/** The backend says `shaderpack`; the shared labels say `shader`. */
const kindLabel = (kind) => projectTypeLabel(kind === 'shaderpack' ? 'shader' : kind);

function summary(report, flagged) {
  const verified = report.files.filter((f) => f.provenance.status === 'verified').length;
  const tone =
    report.critical > 0
      ? { cls: 'scan-critical', iconName: 'alert', title: `${plural(report.critical, 'critical finding')}` }
      : report.warnings > 0
        ? { cls: 'scan-warning', iconName: 'alert', title: `${plural(report.warnings, 'file')} worth a look` }
        : { cls: 'scan-clean', iconName: 'shield', title: 'Nothing suspicious found' };
  return html`
    <div class="scan-summary ${tone.cls}">
      ${icon(tone.iconName)}
      <div class="grow">
        <div class="bold">${tone.title}</div>
        <div class="small scan-dim">
          Checked ${plural(report.examined, 'file')} in ${(report.durationMs / 1000).toFixed(1)}s
          ${report.provenanceChecked ? ` · ${verified} verified as official Modrinth files` : ' · hashes not checked against Modrinth'}
        </div>
        ${report.critical > 0 && html`<div class="small" style="margin-top:4px">Remove or replace the files below before playing this instance.</div>`}
        ${!report.provenanceChecked &&
        html`<div class="small scan-dim" style="margin-top:4px">MineArcade was offline or Modrinth was rate limiting, so tampering could not be checked this time. The content scan above still applies.</div>`}
        ${flagged > 0 && report.critical === 0 && report.warnings === 0 && html`<div class="small scan-dim" style="margin-top:4px">Only informational notes below.</div>`}
      </div>
    </div>`;
}

function provenanceLine(provenance, checked) {
  if (provenance.status === 'verified') {
    return html`<div class="small success-text row" style="gap:6px">${icon('check-circle', 'icon-sm')}
      <span class="truncate">Official Modrinth file${provenance.version ? ` — ${provenance.version}` : ''}</span></div>`;
  }
  if (!checked || provenance.status === 'unchecked') {
    return html`<div class="small muted">Hash not checked against Modrinth.</div>`;
  }
  return html`<div class="small muted">Not in Modrinth’s database — the usual case for CurseForge mods, shader packs and locally built mods, and also what an edited file looks like.</div>`;
}

function findingCard(file, checked) {
  const worst = file.findings.reduce(
    (acc, f) => (SEVERITY_RANK[f.severity] > SEVERITY_RANK[acc] ? f.severity : acc),
    'info',
  );
  return html`
    <div class="scan-file">
      <div class="row-wrap" style="gap:8px">
        <strong class="truncate selectable" title="${file.file}">${fileName(file.file)}</strong>
        <span class="badge">${kindLabel(file.kind)}</span>
        <span class="badge badge-${worst}">${severityLabel(worst)}</span>
        <span class="spacer"></span>
        <span class="small muted">${formatBytes(file.size)}</span>
      </div>
      <div style="margin-top:6px">${provenanceLine(file.provenance, checked)}</div>
      <ul class="scan-findings">
        ${file.findings.map(
          (f) => html`
            <li class="scan-${f.severity}-text">
              ${icon(SEVERITY_ICON[f.severity] ?? 'info', 'icon-sm')}
              <div class="grow">
                <div class="scan-finding-title">${f.summary}</div>
                <div class="small muted">${f.detail}</div>
                ${f.entry && html`<div class="tiny faint mono truncate selectable" title="${f.entry}">${f.entry}</div>`}
              </div>
            </li>`,
        )}
      </ul>
      ${file.truncated &&
      html`<div class="small muted" style="margin-top:8px">Only partly read — very large archives are scanned up to a fixed budget, so a clean result here is not a guarantee for the unread remainder.</div>`}
    </div>`;
}

function cleanList(files, checked) {
  return html`
    <div class="scan-clean-list">
      <div class="bold muted small">${plural(files.length, 'file')} with nothing flagged</div>
      <ul>
        ${files.map(
          (f) => html`
            <li>
              <span class="success-text">${icon('check', 'icon-sm')}</span>
              <span class="truncate selectable">${f.file}</span>
              ${f.provenance.status === 'verified' &&
              html`<span class="muted truncate scan-clean-ver">${f.provenance.version ?? f.provenance.project ?? 'verified'}</span>`}
            </li>`,
        )}
      </ul>
      ${!checked && html`<div class="small muted" style="margin-top:6px">Hashes were not checked against Modrinth this time, so these are clean on content only.</div>`}
    </div>`;
}

/** Open the scan dialog for an instance. It scans straight away (a remembered report would be stale advice). */
export function openScanDialog(instanceId, instanceName) {
  let scanning = false;
  let run = 0;

  const modal = openModal({
    title: `Scan “${instanceName}”`,
    description:
      'Reads every mod, pack and shader for the concrete signs of an account stealer — bundled programs, code that reaches for your Microsoft tokens or Discord — then checks each file’s hash against Modrinth’s database to catch tampering. This is not an antivirus: treat findings as a reason to look closer, not a verdict.',
    size: 'lg',
    body: html`<div data-scan></div>`,
    footer: html`
      <button class="btn btn-outline" data-rescan>${icon('refresh', 'icon-sm')} Rescan</button>
      <button class="btn btn-primary" data-close>Close</button>`,
  });
  const slot = modal.body.querySelector('[data-scan]');
  const rescan = modal.foot.querySelector('[data-rescan]');

  async function scan() {
    if (scanning) return;
    scanning = true;
    const token = ++run;
    rescan.disabled = true;
    setHtml(rescan, html`${spinner('icon-sm')} Scanning…`);
    setHtml(
      slot,
      html`<div class="scan-state">
        ${spinner()}
        <div><div class="bold">Reading every mod and pack…</div><div class="small muted">Then checking each file’s hash against Modrinth.</div></div>
      </div>`,
    );

    try {
      const report = await api.security.scan(instanceId);
      if (modal.closed || token !== run) return;
      const flagged = report.files.filter((f) => f.findings.length > 0);
      const clean = report.files.filter((f) => f.findings.length === 0);
      setHtml(
        slot,
        html`<div class="stack">
          ${summary(report, flagged.length)}
          ${flagged.length > 0 && html`<div class="stack">${flagged.map((f) => findingCard(f, report.provenanceChecked))}</div>`}
          ${clean.length > 0 && cleanList(clean, report.provenanceChecked)}
          ${report.files.length === 0 && html`<p class="muted small center">This instance has no mods, packs or shaders to scan.</p>`}
        </div>`,
      );
    } catch (error) {
      if (modal.closed || token !== run) return;
      setHtml(
        slot,
        html`<div class="scan-state">
          <span class="danger-text">${icon('x-circle', 'icon-lg')}</span>
          <div><div class="bold">The scan could not run</div><div class="small muted selectable">${errorText(error)}</div></div>
          <button class="btn btn-outline btn-sm" data-retry>${icon('refresh', 'icon-sm')} Try again</button>
        </div>`,
      );
    } finally {
      scanning = false;
      if (!modal.closed) {
        rescan.disabled = false;
        setHtml(rescan, html`${icon('refresh', 'icon-sm')} Rescan`);
      }
    }
  }

  rescan.addEventListener('click', scan);
  slot.addEventListener('click', (event) => event.target.closest('[data-retry]') && scan());
  scan();
  return modal;
}
