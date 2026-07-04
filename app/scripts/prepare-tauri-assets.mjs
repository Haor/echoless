#!/usr/bin/env node
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
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

function tryOutput(cmd, cmdArgs, options = {}) {
  try {
    return output(cmd, cmdArgs, options);
  } catch {
    return null;
  }
}

function toolCandidates(name) {
  const ext = process.platform === "win32" ? ".exe" : "";
  const command = process.platform === "win32" && !name.endsWith(".exe") ? `${name}${ext}` : name;
  const envOverride =
    name === "cargo" ? process.env.CARGO : name === "rustc" ? process.env.RUSTC : null;
  const candidates = [envOverride, name, command];
  const pathEnv = process.env.PATH ?? process.env.Path ?? "";
  for (const dir of pathEnv.split(path.delimiter).filter(Boolean)) {
    candidates.push(path.join(dir, command));
  }
  const cargoHomes = [
    process.env.CARGO_HOME,
    process.env.USERPROFILE ? path.join(process.env.USERPROFILE, ".cargo") : null,
    process.env.HOME ? path.join(process.env.HOME, ".cargo") : null,
  ].filter(Boolean);
  for (const home of cargoHomes) {
    candidates.push(path.join(home, "bin", command));
  }
  return [...new Set(candidates.filter(Boolean))];
}

function resolveTool(name, probeArgs = ["--version"]) {
  for (const candidate of toolCandidates(name)) {
    if (tryOutput(candidate, probeArgs) !== null) return candidate;
  }
  return null;
}

function nativeTargetTriple() {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "win32") {
    if (arch === "x64") return "x86_64-pc-windows-msvc";
    if (arch === "arm64") return "aarch64-pc-windows-msvc";
  }
  if (platform === "darwin") {
    if (arch === "arm64") return "aarch64-apple-darwin";
    if (arch === "x64") return "x86_64-apple-darwin";
  }
  if (platform === "linux") {
    if (arch === "x64") return "x86_64-unknown-linux-gnu";
    if (arch === "arm64") return "aarch64-unknown-linux-gnu";
  }
  return null;
}

function rustTargetTriple() {
  const explicit =
    valueOf("--target") ??
    process.env.TAURI_TARGET_TRIPLE ??
    process.env.CARGO_BUILD_TARGET ??
    process.env.TARGET;
  if (explicit) return explicit;
  const native = nativeTargetTriple();
  if (native) return native;
  const rustc = resolveTool("rustc", ["-vV"]);
  if (!rustc) {
    throw new Error("Could not find rustc; pass --target or set TAURI_TARGET_TRIPLE");
  }
  const verbose = output(rustc, ["-vV"]);
  const host = verbose.match(/^host: (.+)$/m)?.[1]?.trim();
  if (!host) throw new Error("Could not determine rust host target triple");
  return host;
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

function copyFile(src, dest) {
  ensureDir(path.dirname(dest));
  if (path.resolve(src) === path.resolve(dest)) {
    console.log(`asset: ${path.relative(repoRoot, dest)} already present`);
    return;
  }
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

function prepareCliSidecar(targetTriple) {
  if (!skipCliBuild) {
    const cargo = resolveTool("cargo");
    if (!cargo) throw new Error("Could not find cargo; ensure Rust is installed and PATH is exported");
    const buildArgs = ["build", "-p", "echoless-cli"];
    if (!dev) buildArgs.push("--release");
    run(cargo, buildArgs);
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

function main() {
  const targetTriple = rustTargetTriple();
  console.log(`Preparing Tauri assets for ${targetTriple} (${profile})`);
  prepareCliSidecar(targetTriple);
  prepareProcessTapHelper();
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
