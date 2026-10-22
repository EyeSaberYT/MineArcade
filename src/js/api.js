// One function per backend command, with the exact argument names the Rust
// side expects (Tauri maps camelCase arguments onto snake_case parameters).
//
// Payload shapes mirror the Rust structs (all camelCase). The important ones:
//   InstanceSummary { id, name, gameVersion, loader, loaderVersion, icon, lastPlayed, playTimeSecs, modCount, installed }
//   Instance { id, name, icon, gameVersion, loader, loaderVersion, group, tags, created, lastPlayed,
//              playTimeSecs, notes, settings: InstanceSettings, installedProjects: InstalledProject[] }
//   InstanceSettings { memoryMin, memoryMax, jvmArgs[], gameArgs[], javaPath, gameDir, width, height,
//                      fullscreen, quickPlay: QuickPlay, env: [k, v][] }
//   QuickPlay { type: 'none' | 'multiplayer' | 'singleplayer' | 'singleplayerContinue', address?, world? }
//   PublicAccount { uuid, username, kind, skinUrl, active }
//   Settings { javaPath, defaultMemoryMin, defaultMemoryMax, defaultJvmArgs[], concurrentDownloads,
//              checkUpdatesOnStart, showSnapshots, closeOnLaunch, keepLauncherOpen,
//              theme: 'dark'|'light'|'system', modrinthApiUrl, featuredServer }
//   Progress { stage, label, current, total, filesDone, filesTotal, finished }
import { invoke } from './tauri.js';

export const auth = {
  accounts: () => invoke('auth_accounts'),
  current: () => invoke('auth_current'),
  /** Resolves once sign-in finishes; the code arrives as an `auth://device-code` event. */
  loginMicrosoft: () => invoke('auth_login_microsoft'),
  logout: (uuid) => invoke('auth_logout', { uuid }),
  setActive: (uuid) => invoke('auth_set_active', { uuid }),
};

export const instances = {
  list: () => invoke('instance_list'),
  get: (id) => invoke('instance_get', { id }),
  /** args: { name, gameVersion, loader, loaderVersion?, icon?, group?, tags?, installLoader? } */
  create: (args) => invoke('instance_create', { args }),
  update: (instance) => invoke('instance_update', { instance }),
  rename: (id, name) => invoke('instance_rename', { id, name }),
  duplicate: (id, name) => invoke('instance_duplicate', { id, name: name ?? null }),
  remove: (id, deleteFiles) => invoke('instance_delete', { id, deleteFiles }),
  installLoader: (id, loaderVersion) =>
    invoke('instance_install_loader', { id, loaderVersion: loaderVersion ?? null }),
  /** ContentEntry[] { name, path, enabled, size, kind, projectId, versionId, title, iconUrl } */
  content: (id) => invoke('instance_content', { id }),
  toggleContent: (id, path, enabled) => invoke('instance_toggle_content', { id, path, enabled }),
  deleteContent: (id, path) => invoke('instance_delete_content', { id, path }),
  importFiles: (id, paths, folder) =>
    invoke('instance_import_files', { id, paths, folder: folder ?? null }),
  paths: (id) => invoke('instance_paths', { id }),
  contentDirName: (projectType) => invoke('instance_content_dir_name', { projectType }),
  defaultSettings: () => invoke('instance_default_settings'),
  slug: (name) => invoke('instance_slug', { name }),
};

export const importApi = {
  /** ImportScan { sources: ImportSourceInfo[], candidates: ImportCandidate[] } */
  scan: () => invoke('import_scan'),
  inspect: (path) => invoke('import_inspect', { path }),
  /** requests: ImportRequest[] { source, name, path, gameVersion, loader, loaderVersion, icon } */
  importInstances: (requests) => invoke('import_instances', { requests }),
};

export const modpacks = {
  /** ModpackPreview { name, summary, versionId, gameVersion, loader, loaderVersion, fileCount, totalSize, dependencies } */
  preview: (path) => invoke('modpack_preview', { path }),
  import: (path, name) => invoke('modpack_import', { path, name: name ?? null }),
  export: (id, target) => invoke('modpack_export', { id, target }),
};

export const servers = {
  /** ServerListFile { path, exists, servers: ServerEntry[] { name, address, icon, hidden, acceptTextures } } */
  list: (id) => invoke('servers_list', { id }),
  save: (id, list) => invoke('servers_save', { id, servers: list }),
};

export const worlds = {
  /** WorldsOverview { savesPath, backupsPath, worlds: World[], backups: WorldBackup[] } */
  overview: (id) => invoke('worlds_overview', { id }),
  backup: (id, world) => invoke('world_backup_create', { id, world }),
  /** RestoredWorld { world, safety } */
  restore: (id, file) => invoke('world_backup_restore', { id, file }),
  deleteBackup: (id, file) => invoke('world_backup_delete', { id, file }),
};

export const screenshots = {
  /** ScreenshotsFile { path, exists, screenshots: Screenshot[] { name, path, size, taken } } */
  list: (id) => invoke('screenshots_list', { id }),
  remove: (id, name) => invoke('screenshot_delete', { id, name }),
};

export const versions = {
  /** VersionOption[] { id, versionType, releaseTime, latestRelease } */
  list: (includeSnapshots = false) => invoke('versions_list', { includeSnapshots }),
  refresh: () => invoke('versions_refresh'),
  latest: () => invoke('versions_latest'),
  /** LoaderVersion[] { version, stable, gameVersion } */
  loaderVersions: (loaderKind, gameVersion) => invoke('loader_versions', { loaderKind, gameVersion }),
};

export const launch = {
  /** Resolves with LaunchedProcess { pid, logFile, command, arguments } once Java has started. */
  instance: (id, quickPlay) => invoke('launch_instance', { id, quickPlay: quickPlay ?? null }),
  preview: (id) => invoke('launch_preview', { id }),
  logs: (id, lines) => invoke('launch_logs', { id, lines: lines ?? null }),
  markStopped: (id) => invoke('launch_mark_stopped', { id }),
  running: () => invoke('launch_running'),
  stop: (id) => invoke('launch_stop', { id }),
};

export const modrinth = {
  /**
   * Search a catalog. filters: { query, projectType?, gameVersion?, loader?, categories[], index? }.
   * source: 'modrinth' (default) or 'curseforge'. Returns SearchResponse { hits, offset, limit, totalHits }.
   */
  search: (filters, limit = 20, offset = 0, source) =>
    invoke('modrinth_search', {
      filters,
      source: source && source !== 'modrinth' ? source : null,
      limit,
      offset,
    }),
  project: (id) => invoke('modrinth_project', { id }),
  projects: (ids) => invoke('modrinth_projects', { ids }),
  versions: (projectId, gameVersion, loader) =>
    invoke('modrinth_versions', { projectId, gameVersion: gameVersion ?? null, loader: loader ?? null }),
  tags: (kind) => invoke('modrinth_tags', { kind }),
  install: (instanceId, projectId, versionId) =>
    invoke('modrinth_install', { instanceId, projectId, versionId: versionId ?? null }),
  installModpack: (projectId, versionId, name) =>
    invoke('modrinth_install_modpack', { projectId, versionId: versionId ?? null, name: name ?? null }),
  /** UpdateInfo[] { projectId, title, iconUrl, currentVersionId, latestVersionId, latestVersionName, changelog, projectType } */
  checkUpdates: (instanceId) => invoke('modrinth_check_updates', { instanceId }),
  applyUpdate: (instanceId, projectId, versionId) =>
    invoke('modrinth_apply_update', { instanceId, projectId, versionId }),
  updateAll: (instanceId) => invoke('modrinth_update_all', { instanceId }),
  dependencies: (instanceId, versionId) => invoke('modrinth_dependencies', { instanceId, versionId }),
};

export const java = {
  /** JavaRuntime[] { path, version, major, source, vendor } */
  list: () => invoke('java_list'),
  detect: (requiredMajor = 0) => invoke('java_detect', { requiredMajor }),
  install: (requiredMajor) => invoke('java_install', { requiredMajor }),
  requirement: (gameVersion) => invoke('java_requirement', { gameVersion }),
};

export const security = {
  /** ScanReport (see dialogs/scan.js) */
  scan: (id) => invoke('scan_instance', { id }),
};

export const system = {
  getSettings: () => invoke('settings_get'),
  saveSettings: (settings) => invoke('settings_save', { settings }),
  /** AppInfo { name, version, os, arch, dataDir, instancesDir, javaDir, cacheDir, modrinthApiUrl } */
  appInfo: () => invoke('app_info'),
  reveal: (path) => invoke('system_reveal', { path }),
  openUrl: (url) => invoke('system_open_url', { url }),
};

export const skin = {
  /** SkinCurrentResponse { account: PublicAccount | null, skin: { skinUrl, variant, capeUrl } } */
  current: () => invoke('skin_current'),
  upload: (path, variant = 'classic') => invoke('skin_upload', { path, variant }),
};
