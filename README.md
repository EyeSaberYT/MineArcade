# MineArcade Launcher

The official launcher for the **MineArcade** network (`play.minearcade.org`): a fast, ad-free
Minecraft: Java Edition launcher built with [Tauri 2](https://tauri.app). It has a Rust core and a
plain HTML/CSS/JavaScript interface, with no frontend build step.

## Features

**Playing**
- One-click **Join MineArcade**: launches straight into `play.minearcade.org`, using Quick Play on 1.20+ and `--server` on older versions.
  New instances get the server added to their in-game multiplayer list automatically.
- **Microsoft sign-in** (device code flow) with multiple accounts and automatic token refresh.
- Every Minecraft version: releases, snapshots and old betas/alphas.
- **Fabric, Quilt, Forge and NeoForge**, installed automatically.
- **Automatic Java**: the exact Mojang runtime each version needs is downloaded for you.
- Live game console, a crash/log viewer, and a Stop button for running games.
- Log4Shell protection for 1.7–1.18 (Mojang's patched logging config).

**Instances**
- Create, rename, duplicate and delete instances, with custom icons and notes.
- Per-instance memory, JVM/game arguments, Java path, window size, fullscreen and Quick Play.
- Content manager for mods, resource packs, shaders and datapacks: enable/disable, delete, and drag-and-drop `.jar` / `.zip` files.
- Update checks for installed mods, with one-click apply.
- Server list (`servers.dat`) editor, screenshot browser, and world backups you can restore.
- Import and export `.mrpack` modpacks, and install CurseForge `.zip` packs.
- **Import from other launchers**: Prism, MultiMC, CurseForge, the vanilla launcher and the Modrinth App.
- **Mod safety scan**: a heuristic check of mod jars for account-stealer patterns, plus Modrinth provenance.

**Content**
- Browse **Modrinth** and **CurseForge**: mods, modpacks, resource packs, shaders and datapacks.
- Filter by version, loader, category and sort order, then install with automatic dependency resolution.

**Account & app**
- Skin viewer (3D) and skin upload.
- Dark, light and system themes.
- Signed **in-app updates** from GitHub Releases.

## Project layout

```
src/                     Frontend, served as-is by Tauri
  index.html, styles.css
  css/                   Area styles (home, instance, discover, settings)
  js/
    main.js              Bootstrap: sidebar, backend events, routes
    api.js               One wrapper per backend command (payload shapes documented)
    store.js, router.js  Reactive state, hash router
    ui.js                Templating, icons, toasts, modals, menus, formatters
    actions.js           Shared flows (launch, stop, sign in, settings)
    pages/               home, library, instance, discover, project, settings
    dialogs/             new instance, install, scan, skin, imports, icon picker
  vendor/                skinview3d, marked, DOMPurify (npm run vendor)
src-tauri/               Rust backend
  src/core/              Launcher logic (auth, minecraft, java, download, instances, modpacks, servers, …)
  src/api/               Modrinth and CurseForge clients
  src/commands/          Tauri command surface
scripts/                 Icon generation, vendoring, JS checks
```

Launcher data lives in `%APPDATA%\.minearcade` (override with `MINEARCADE_DATA_DIR`).

## Requirements (Windows)

1. **Microsoft C++ Build Tools** with the "Desktop development with C++" workload.
2. **Rust (MSVC toolchain)** via rustup (`rustup default stable-msvc`). If a Chocolatey GNU Rust is
   also installed, put `%USERPROFILE%\.cargo\bin` before `C:\ProgramData\chocolatey\bin` in your
   `PATH` (or `choco uninstall rust`); the GNU toolchain cannot build this project.
3. **Node.js** for the Tauri CLI.
4. **WebView2**, which is preinstalled on Windows 10/11.

## Develop

```powershell
npm install
npm run dev        # run the app
npm run check      # syntax- and import-check the frontend modules
cd src-tauri; cargo test
```

## Microsoft sign-in

MineArcade signs in with its own Azure application (client) ID, which Mojang has allow-listed.
The ID is **compiled into the app** and can't be changed from the UI. To build with a different
registration, set it at build time:

```powershell
$env:MINEARCADE_MS_CLIENT_ID = "<client-id>"; npm run build
```

## Releases & auto-update

Updates are signed. The private key lives **outside the repo** at
`%USERPROFILE%\.tauri\minearcade.key`; its public half is in `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`).

1. Create the GitHub repository the updater checks (`EyeSaberYT/MineArcade`, as set in
   `plugins.updater.endpoints`; change it there if you use another name).
2. Add the repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the key file's contents) and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (empty).
3. Bump the version in `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, then
   push a tag: `git tag v0.2.0; git push origin v0.2.0`. The workflow builds Windows, Linux and macOS
   installers and publishes `latest.json`.

To build locally with signing: set `TAURI_SIGNING_PRIVATE_KEY` to the key file's contents and run
`npm run build`.

**Back up the private key.** Without it you can't ship updates to existing installs.

## Credits

The launcher core is based on [Aether](https://github.com/EyeSaberYT/Aether) (MIT).
