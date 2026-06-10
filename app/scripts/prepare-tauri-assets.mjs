#!/usr/bin/env node
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const scriptDir = path.dirname(new URL(import.meta.url).pathname);
const appDir = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(appDir, "..");
const srcTauriDir = path.join(appDir, "src-tauri");

const args = process.argv.slice(2);
const has = (flag) => args.includes(flag);
const valueOf = (flag) => {
  const i = args.indexOf(flag);
  return i >= 0 && i + 1 < args.length ? args[i + 1] : null;
};

const dev = has("--dev");
const skipCliBuild = has("--skip-cli-build");
const skipHelperBuild = has("--skip-helper-build");
const requireLocalvqe = has("--require-localvqe-assets");
const profile = dev ? "debug" : "release";

function run(cmd, cmdArgs, options = {}) {
  const result = spawnSync(cmd, cmdArgs, {
    cwd: options.cwd ?? repoRoot,
    stdio: "inherit",
    shell: false,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${cmd} ${cmdArgs.join(" ")} failed with ${result.status}`);
  }
}

function output(cmd, cmdArgs, options = {}) {
  return execFileSync(cmd, cmdArgs, {
    cwd: options.cwd ?? repoRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

function rustTargetTriple() {
  const explicit =
    valueOf("--target") ??
    process.env.TAURI_TARGET_TRIPLE ??
    process.env.CARGO_BUILD_TARGET ??
    process.env.TARGET;
  if (explicit) return explicit;
  const verbose = output("rustc", ["-vV"]);
  const host = verbose.match(/^host: (.+)$/m)?.[1]?.trim();
  if (!host) throw new Error("Could not determine rust host target triple");
  return host;
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function copyFile(src, dest) {
  ensureDir(path.dirname(dest));
  fs.copyFileSync(src, dest);
  if (process.platform !== "win32") {
    const mode = fs.statSync(src).mode;
    fs.chmodSync(dest, mode | 0o755);
  }
  console.log(`asset: ${path.relative(repoRoot, dest)} <- ${path.relative(repoRoot, src)}`);
}

function existsFile(p) {
  return Boolean(p) && fs.existsSync(p) && fs.statSync(p).isFile();
}

function* walkFiles(root) {
  if (!root || !fs.existsSync(root)) return;
  const entries = fs.readdirSync(root, { withFileTypes: true });
  for (const entry of entries) {
    const p = path.join(root, entry.name);
    if (entry.isDirectory()) {
      yield* walkFiles(p);
    } else if (entry.isFile()) {
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

function prepareCliSidecar(targetTriple) {
  if (!skipCliBuild) {
    const buildArgs = ["build", "-p", "echoless-cli"];
    if (!dev) buildArgs.push("--release");
    run("cargo", buildArgs);
  }

  const ext = process.platform === "win32" ? ".exe" : "";
  const cli = path.join(repoRoot, "target", profile, `echoless${ext}`);
  if (!existsFile(cli)) {
    throw new Error(`CLI binary not found after build: ${cli}`);
  }

  const dest = path.join(
    srcTauriDir,
    "binaries",
    `echoless-${targetTriple}${ext}`,
  );
  copyFile(cli, dest);
}

function prepareProcessTapHelper() {
  if (process.platform !== "darwin") return;

  const envHelper = process.env.ECHOLESS_PROCESS_TAP_HELPER;
  let helper = existsFile(envHelper) ? envHelper : null;

  const toolDir = path.join(repoRoot, "tools", "macos-process-tap-poc");
  if (!helper && !skipHelperBuild && fs.existsSync(path.join(toolDir, "build.sh"))) {
    run("bash", ["build.sh"], { cwd: toolDir });
  }
  if (!helper) {
    const built = path.join(toolDir, ".build", "echoless-process-tap-poc");
    helper = existsFile(built) ? built : null;
  }
  if (!helper) {
    console.warn("asset warning: Process Tap helper not found; macOS system reference will need ECHOLESS_PROCESS_TAP_HELPER");
    return;
  }

  copyFile(
    helper,
    path.join(srcTauriDir, "resources", "helpers", "echoless-process-tap-poc"),
  );
}

function prepareLocalvqeModel() {
  const modelName = "localvqe-v1.3-4.8M-f32.gguf";
  const candidates = [
    process.env.ECHOLESS_LOCALVQE_MODEL,
    process.env.LOCALVQE_MODEL,
    process.env.RUNNER_TEMP
      ? path.join(
          process.env.RUNNER_TEMP,
          "localvqe-regression-build",
          "bench_assets",
          modelName,
        )
      : null,
  ].filter(Boolean);

  let model = candidates.find(existsFile) ?? null;
  if (!model && process.env.RUNNER_TEMP) {
    model = firstFile(process.env.RUNNER_TEMP, (file) => path.basename(file) === modelName);
  }

  if (!model) {
    const message = `LocalVQE model ${modelName} not found; bundled model will be absent`;
    if (requireLocalvqe) throw new Error(message);
    console.warn(`asset warning: ${message}`);
    return false;
  }

  copyFile(
    model,
    path.join(srcTauriDir, "resources", "localvqe", "models", modelName),
  );
  return true;
}

function localvqeLibraryName(file) {
  const name = path.basename(file);
  if (process.platform === "win32") return name.toLowerCase() === "localvqe.dll";
  if (process.platform === "darwin") return name.startsWith("liblocalvqe") && name.endsWith(".dylib");
  return name.startsWith("liblocalvqe") && name.includes(".so");
}

function companionLibrary(file) {
  const name = path.basename(file).toLowerCase();
  if (process.platform === "win32") return name.endsWith(".dll");
  return name.endsWith(".dylib") || name.includes(".so");
}

function prepareLocalvqeNative() {
  let library = existsFile(process.env.ECHOLESS_LOCALVQE_LIBRARY)
    ? process.env.ECHOLESS_LOCALVQE_LIBRARY
    : null;
  if (!library && process.env.RUNNER_TEMP) {
    library = firstFile(process.env.RUNNER_TEMP, localvqeLibraryName);
  }

  if (!library) {
    const message = "LocalVQE native library not found; bundled LocalVQE runtime will be absent";
    if (requireLocalvqe) throw new Error(message);
    console.warn(`asset warning: ${message}`);
    return false;
  }

  const nativeDir = path.join(srcTauriDir, "resources", "localvqe", "native");
  ensureDir(nativeDir);
  for (const file of fs.readdirSync(path.dirname(library)).map((name) => path.join(path.dirname(library), name))) {
    if (existsFile(file) && companionLibrary(file)) {
      copyFile(file, path.join(nativeDir, path.basename(file)));
    }
  }
  return true;
}

function main() {
  const targetTriple = rustTargetTriple();
  console.log(`Preparing Tauri assets for ${targetTriple} (${profile})`);
  prepareCliSidecar(targetTriple);
  prepareProcessTapHelper();
  const hasModel = prepareLocalvqeModel();
  const hasNative = prepareLocalvqeNative();
  if (requireLocalvqe && (!hasModel || !hasNative)) {
    throw new Error("LocalVQE assets are incomplete");
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
