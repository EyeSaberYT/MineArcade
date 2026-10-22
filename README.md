# MineArcade Launcher

A Minecraft: Java Edition launcher built with [Tauri 2](https://tauri.app): a Rust backend and a plain HTML/CSS/JavaScript frontend with no frontend build step.

## Features

- **Microsoft sign-in** using the device code flow (Microsoft → Xbox Live → XSTS → Minecraft), with automatic token refresh and support for multiple accounts.
- **Every vanilla version**: releases, snapshots, and old beta/alpha versions from Mojang's version manifest.
- **Fabric**: install with one click and choose the loader version.
- **Instances**: each one has its own game folder, version, memory, JVM arguments and optional custom Java.
- **Automatic Java**: downloads the exact Java runtime Mojang ships for each version (Java 8 / 17 / 21 / …), so players never install Java themselves.
- **Auto-join a server**: set a per-instance server, or a featured server that gets a "Join" button. Uses Quick Play on 1.20+ and `--server` on older versions.
- **Downloads**: parallel, retried, and checked against SHA-1 checksums, with a progress bar.
- **Live game output**: Mojang's XML log format is converted back to readable lines. Stop and crash detection are included.
- Applies Mojang's Log4Shell-safe logging config.

## Project layout

```
src/                  Frontend (served as-is by Tauri)
  index.html
  styles.css
  main.js
  assets/logo.png
src-tauri/            Rust backend
  src/
    lib.rs            App setup and command registration
    commands.rs       Commands the frontend calls
    auth.rs           Microsoft / Xbox / Minecraft authentication
    versions.rs       Version manifest, version JSON, rules, libraries, assets
    fabric.rs         Fabric meta API
    java.rs           Mojang Java runtime downloads
    download.rs       Parallel verified downloader
    launch.rs         Launch pipeline, process handling, log parsing
    config.rs         settings.json / instances.json / accounts.json
    paths.rs          Data directory layout
scripts/gen-icons.mjs Generates the pixel-art icon set
```

Launcher data lives in `%APPDATA%\MineArcade` (`versions`, `libraries`, `assets`, `runtime`, `instances`, …).

## Requirements (Windows)

1. **Microsoft C++ Build Tools**: the "Desktop development with C++" workload.
   `winget install Microsoft.VisualStudio.2022.BuildTools` (then add the C++ workload in the installer)
2. **Rust (MSVC toolchain)** via rustup: `winget install Rustlang.Rustup`, then `rustup default stable-msvc`.
   The GNU toolchain (for example from Chocolatey) needs a full MinGW install and isn't recommended for Tauri.
3. **Node.js**, used for the Tauri CLI.
4. **WebView2**, which is already installed on Windows 10/11.

## Run it

```powershell
npm install
npm run dev      # development window with hot-reload of the frontend
npm run build    # release build + NSIS installer in src-tauri/target/release/bundle
```

## Set up Microsoft sign-in (required)

Microsoft doesn't let launchers share a sign-in app, so you need your own **Azure application (client) ID**:

1. Go to the [Azure portal → App registrations](https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade) → **New registration**.
   - Name: `MineArcade`
   - Supported account types: **Personal Microsoft accounts only**
   - Redirect URI: leave empty (the device code flow doesn't use one)
2. In the app's **Authentication** page, set **Allow public client flows** to **Yes** and save.
3. Copy the **Application (client) ID** from the Overview page.
4. **Ask Mojang for Minecraft API access** using the form at <https://aka.ms/mce-reviewappid>. Until the ID is approved, sign-in stops at the last step with "Minecraft services rejected the sign-in".
5. Paste the ID into **Settings → Azure application (client) ID**. You can also bake it into the build:
   ```powershell
   $env:MINEARCADE_CLIENT_ID = "your-client-id"; npm run build
   ```

Accounts (refresh and access tokens) are stored in `%APPDATA%\MineArcade\accounts.json` and are never sent to the frontend.

## Regenerate icons

```powershell
npm run icons
```
