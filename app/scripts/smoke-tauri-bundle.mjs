#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const appDir = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(appDir, "..");
const srcTauriDir = path.join(appDir, "src-tauri");

const args = process.argv.slice(2);
const valueOf = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : null;
};

function existsFile(p) {
  return Boolean(p) && fs.existsSync(p) && fs.statSync(p).isFile();
}

function existsDir(p) {
  return Boolean(p) && fs.existsSync(p) && fs.statSync(p).isDirectory();
}

function assertFile(p, label) {
  if (!existsFile(p)) throw new Error(`${label} missing: ${p}`);
  return p;
}

function assertExecutable(p, label) {
  assertFile(p, label);
  if (process.platform !== "win32") {
    fs.accessSync(p, fs.constants.X_OK);
  }
  return p;
}

function isWalkableFile(p, entry) {
  if (entry.isFile()) return true;
  if (!entry.isSymbolicLink()) return false;
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

function* walkFiles(root) {
  if (!root || !fs.existsSync(root)) return;
  const entries = fs.readdirSync(root, { withFileTypes: true });
  for (const entry of entries) {
    const p = path.join(root, entry.name);
    if (entry.isDirectory()) {
      yield* walkFiles(p);
    } else if (isWalkableFile(p, entry)) {
      yield p;
    }
  }
}

function firstFile(root, predicate) {
  const matches = [];
  for (const file of walkFiles(root)) {
    if (predicate(file)) matches.push(file);
  }
  matches.sort();
  return matches[0] ?? null;
}

function firstDirectFile(root, predicate) {
  if (!root || !fs.existsSync(root)) return null;
  const matches = [];
  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const p = path.join(root, entry.name);
    if (isWalkableFile(p, entry) && predicate(p)) matches.push(p);
  }
  matches.sort();
  return matches[0] ?? null;
}

function basenameLower(file) {
  return path.basename(file).toLowerCase();
}

function discoverLayout() {
  const explicitInstalledApp = valueOf("--installed-app");
  const explicitApp = valueOf("--app");
  const explicitTargetDir = valueOf("--target-dir");
  if (explicitInstalledApp) return installedAppLayout(path.resolve(explicitInstalledApp));
  if (explicitApp) return macAppLayout(path.resolve(explicitApp));
  if (explicitTargetDir) return targetDirLayout(path.resolve(explicitTargetDir));

  if (process.platform === "darwin") {
    const appBundle = path.join(
      srcTauriDir,
      "target",
      "debug",
      "bundle",
      "macos",
      "Echoless.app",
    );
    if (existsDir(appBundle)) return macAppLayout(appBundle);
  }

  return targetDirLayout(path.join(srcTauriDir, "target", "debug"));
}

function macAppLayout(appBundle) {
  return {
    kind: "macos-app",
    root: appBundle,
    appExecutable: path.join(appBundle, "Contents", "MacOS", "echoless-app"),
    cli: path.join(appBundle, "Contents", "MacOS", "echoless"),
    resources: path.join(appBundle, "Contents", "Resources", "resources"),
    infoPlist: path.join(appBundle, "Contents", "Info.plist"),
    helper: path.join(
      appBundle,
      "Contents",
      "Resources",
      "resources",
      "helpers",
      "echoless-process-tap-poc",
    ),
  };
}

function targetDirLayout(targetDir) {
  const ext = process.platform === "win32" ? ".exe" : "";
  const helperName =
    process.platform === "darwin" ? "echoless-process-tap-poc" : "echoless-process-tap-poc";
  return {
    kind: "target-dir",
    root: targetDir,
    appExecutable: path.join(targetDir, `echoless-app${ext}`),
    cli: path.join(targetDir, `echoless${ext}`),
    resources: path.join(targetDir, "resources"),
    infoPlist: null,
    helper: path.join(targetDir, "resources", "helpers", helperName),
  };
}

function installedAppLayout(installDir) {
  const resources = discoverResourcesDir(installDir);
  const appExecutable =
    firstDirectFile(
      installDir,
      (file) =>
        ["echoless.exe", "echoless-app.exe", "echoless app.exe", "echoless_gui.exe"].includes(
          basenameLower(file),
        ),
    ) ??
    firstDirectFile(
      installDir,
      (file) => basenameLower(file).endsWith(".exe") && basenameLower(file).includes("echoless"),
    ) ??
    firstFile(
      installDir,
      (file) =>
        ["echoless.exe", "echoless-app.exe", "echoless app.exe", "echoless_gui.exe"].includes(
          basenameLower(file),
        ),
    ) ??
    firstFile(
      installDir,
      (file) =>
        basenameLower(file).endsWith(".exe") && basenameLower(file).includes("echoless"),
    );
  const cli =
    firstFile(
      resources,
      (file) =>
        (basenameLower(file) === "echoless.exe" ||
          (basenameLower(file).startsWith("echoless-") && basenameLower(file).endsWith(".exe"))) &&
        (!appExecutable || path.resolve(file) !== path.resolve(appExecutable)),
    ) ??
    firstFile(
      installDir,
      (file) =>
        basenameLower(file) === "echoless.exe" &&
        (!appExecutable || path.resolve(file) !== path.resolve(appExecutable)),
    ) ??
    firstFile(
      installDir,
      (file) =>
        basenameLower(file).startsWith("echoless-") &&
        basenameLower(file).endsWith(".exe") &&
        (!appExecutable || path.resolve(file) !== path.resolve(appExecutable)),
    );
  return {
    kind: "windows-installed-app",
    root: installDir,
    appExecutable,
    cli,
    resources,
    infoPlist: null,
    helper: null,
  };
}

function discoverResourcesDir(root) {
  const direct = path.join(root, "resources");
  return direct;
}

function run(cmd, cmdArgs, options = {}) {
  const result = spawnSync(cmd, cmdArgs, {
    cwd: options.cwd ?? repoRoot,
    env: options.env ?? process.env,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const stdout = result.stdout ? `\nstdout:\n${result.stdout}` : "";
    const stderr = result.stderr ? `\nstderr:\n${result.stderr}` : "";
    throw new Error(`${cmd} ${cmdArgs.join(" ")} failed with ${result.status}${stdout}${stderr}`);
  }
  return result;
}

function smoke() {
  const layout = discoverLayout();
  console.log(`bundle-smoke: layout=${layout.kind}`);
  console.log(`bundle-smoke: root=${layout.root}`);

  assertExecutable(layout.appExecutable, "Tauri app executable");
  assertExecutable(layout.cli, "Echoless sidecar CLI");
  if (!existsDir(layout.resources)) throw new Error(`resources directory missing: ${layout.resources}`);

  if (layout.helper) {
    assertExecutable(layout.helper, "Process Tap helper");
  }
  if (layout.infoPlist) {
    const plist = fs.readFileSync(layout.infoPlist, "utf8");
    for (const key of ["NSMicrophoneUsageDescription", "NSAudioCaptureUsageDescription"]) {
      if (!plist.includes(key)) throw new Error(`Info.plist missing ${key}`);
    }
  }

  const processors = run(layout.cli, ["processors", "--json"], { capture: true });
  const manifest = JSON.parse(processors.stdout);
  if (!manifest.processors?.some((p) => p.kind === "localvqe")) {
    throw new Error("processor manifest does not include localvqe");
  }

  console.log("bundle-smoke: ok");
}

try {
  smoke();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
