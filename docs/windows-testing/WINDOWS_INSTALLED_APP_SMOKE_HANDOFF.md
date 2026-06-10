# Windows Installed-App Smoke Handoff

## Goal

Prove the remaining audit items:

- `RUNTIME-2`: LocalVQE native runtime and bundled model are actually usable from the Tauri-installed app.
- `PKG-1`: the installed GUI package contains the Echoless CLI sidecar and resources in the layout the app expects.

This is a packaging/runtime smoke. It does not replace AEC3 / LocalVQE subjective audio quality testing.

## What This Smoke Checks

`app/scripts/smoke-windows-installed-app.ps1` is the preferred Windows wrapper:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-windows-installed-app.ps1
```

It finds the generated NSIS/MSI installer under `src-tauri\target\debug\bundle`, installs it silently, locates the installed Echoless directory, and then delegates to `smoke-tauri-bundle.mjs --installed-app`.

`app/scripts/smoke-tauri-bundle.mjs` now supports:

```powershell
node .\scripts\smoke-tauri-bundle.mjs --installed-app "<INSTALL_DIR>"
```

For a Windows installed app directory it verifies:

- Tauri app executable exists.
- Echoless sidecar CLI exists.
- `resources\localvqe\models\localvqe-v1.3-4.8M-f32.gguf` exists.
- `resources\localvqe\native\localvqe.dll` exists.
- `echoless.exe processors --json` reports `localvqe`.
- A synthetic offline LocalVQE run can load the bundled model and DLL and write a non-empty output WAV.

The smoke does not need microphone permission, system audio capture permission, VB-CABLE, or physical audio devices.

## Build And Install

From the repository root on Windows:

```powershell
pnpm -C app install
pnpm -C app prepare:tauri-assets --require-localvqe-assets
pnpm -C app tauri build --debug --ci --bundles nsis
```

The generated NSIS installer is usually under:

```text
app\src-tauri\target\debug\bundle\
```

If testing a release build, use the matching release bundle and installed directory instead.

## Run Installer + Installed-App Smoke

From `app\`:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-windows-installed-app.ps1
```

Optional arguments:

```powershell
# Use a custom bundle output root.
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-windows-installed-app.ps1 `
  -BundleRoot ".\src-tauri\target\debug\bundle"

# Skip installer execution and only smoke an already-installed app.
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke-windows-installed-app.ps1 `
  -SkipInstall `
  -InstallDir "$env:LOCALAPPDATA\Programs\Echoless"
```

## Locate Install Directory

Common candidates:

```powershell
$candidates = @(
  "$env:LOCALAPPDATA\Programs\Echoless",
  "$env:ProgramFiles\Echoless",
  "$env:ProgramFiles(x86)\Echoless"
)
$candidates | Where-Object { Test-Path $_ }
```

If none match, search:

```powershell
Get-ChildItem "$env:LOCALAPPDATA\Programs", "$env:ProgramFiles", "$env:ProgramFiles(x86)" `
  -Directory -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -like "*Echoless*" } |
  Select-Object -ExpandProperty FullName
```

## Run Installed-App Smoke

If the wrapper cannot locate the app, manually locate the install directory and run:

```powershell
node .\scripts\smoke-tauri-bundle.mjs --installed-app "$env:LOCALAPPDATA\Programs\Echoless"
```

Expected success tail:

```text
bundle-smoke: layout=windows-installed-app
bundle-smoke: root=...
bundle-smoke: localvqe output=...
bundle-smoke: ok
```

If the installed directory is different, pass that path explicitly.

## Optional Debug Target Smoke

Before installing, this checks the uninstalled debug layout:

```powershell
node .\scripts\smoke-tauri-bundle.mjs --target-dir .\src-tauri\target\debug
```

This is useful but does not close `PKG-1` by itself. `PKG-1` requires the installed-app smoke above.

## Evidence To Report

Add a section to `WINDOWS_AEC3_LOCALVQE_TEST_RESULTS.md` or a dedicated result file with:

- Commit SHA.
- Tauri build command.
- Installer path.
- Installed app directory.
- Full `smoke-windows-installed-app.ps1` console output, including the delegated `smoke-tauri-bundle.mjs --installed-app ...` output.
- Whether `bundle-smoke: ok` appeared.
- If failed: missing file path or command stderr.

## Close Conditions

Only mark the audit rows done after Windows evidence shows:

- Installed app smoke passes with `bundle-smoke: ok`.
- The smoke used the installed directory, not only `target\debug`.
- The LocalVQE offline output WAV is non-empty.
- The installed directory contains both the GUI executable and sidecar CLI.
