#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
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

function discoverLayout() {
  const explicitApp = valueOf("--app");
  const explicitTargetDir = valueOf("--target-dir");
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

function localvqeLibraryName(file) {
  const name = path.basename(file);
  if (process.platform === "win32") return name.toLowerCase() === "localvqe.dll";
  if (process.platform === "darwin") return name.startsWith("liblocalvqe") && name.endsWith(".dylib");
  return name.startsWith("liblocalvqe") && name.includes(".so");
}

function writeSyntheticWav(file, samples, rate) {
  const dataBytes = samples.length * 2;
  const buffer = Buffer.alloc(44 + dataBytes);
  buffer.write("RIFF", 0);
  buffer.writeUInt32LE(36 + dataBytes, 4);
  buffer.write("WAVE", 8);
  buffer.write("fmt ", 12);
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(1, 22);
  buffer.writeUInt32LE(rate, 24);
  buffer.writeUInt32LE(rate * 2, 28);
  buffer.writeUInt16LE(2, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write("data", 36);
  buffer.writeUInt32LE(dataBytes, 40);
  samples.forEach((sample, i) => {
    const clamped = Math.max(-1, Math.min(1, sample));
    buffer.writeInt16LE(Math.round(clamped * 32767), 44 + i * 2);
  });
  fs.writeFileSync(file, buffer);
}

function makeSmokeWavs(dir) {
  const rate = 48000;
  const seconds = 2;
  const frames = rate * seconds;
  const ref = new Array(frames);
  const mic = new Array(frames);
  for (let n = 0; n < frames; n += 1) {
    const t = n / rate;
    const far = 0.2 * Math.sin(2 * Math.PI * 440 * t);
    const near = t >= 0.55 && t <= 1.55 ? 0.08 * Math.sin(2 * Math.PI * 180 * t) : 0;
    const echo = n >= 960 ? ref[n - 960] * 0.35 : 0;
    ref[n] = far;
    mic[n] = near + echo;
  }
  const micPath = path.join(dir, "smoke_mic.wav");
  const refPath = path.join(dir, "smoke_ref.wav");
  writeSyntheticWav(micPath, mic, rate);
  writeSyntheticWav(refPath, ref, rate);
  return { micPath, refPath };
}

function tomlString(value) {
  return JSON.stringify(value);
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

  if (process.platform === "darwin") {
    assertExecutable(layout.helper, "Process Tap helper");
    if (layout.infoPlist) {
      const plist = fs.readFileSync(layout.infoPlist, "utf8");
      for (const key of ["NSMicrophoneUsageDescription", "NSAudioCaptureUsageDescription"]) {
        if (!plist.includes(key)) throw new Error(`Info.plist missing ${key}`);
      }
    }
  }

  const localvqeRoot = path.join(layout.resources, "localvqe");
  const model = path.join(localvqeRoot, "models", "localvqe-v1.3-4.8M-f32.gguf");
  const nativeDir = path.join(localvqeRoot, "native");
  const library = firstFile(nativeDir, localvqeLibraryName);
  assertFile(model, "LocalVQE bundled model");
  assertFile(library, "LocalVQE native library");

  const processors = run(layout.cli, ["processors", "--json"], { capture: true });
  const manifest = JSON.parse(processors.stdout);
  if (!manifest.processors?.some((p) => p.kind === "localvqe")) {
    throw new Error("processor manifest does not include localvqe");
  }

  const smokeDir = fs.mkdtempSync(path.join(os.tmpdir(), "echoless-tauri-bundle-smoke-"));
  const { micPath, refPath } = makeSmokeWavs(smokeDir);
  const outPath = path.join(smokeDir, "smoke_localvqe_out.wav");
  const configPath = path.join(smokeDir, "localvqe-bundle-smoke.toml");
  fs.writeFileSync(
    configPath,
    [
      `mic = ${tomlString(micPath)}`,
      `reference = ${tomlString(refPath)}`,
      `output = ${tomlString(outPath)}`,
      "sample_rate = 48000",
      "frame_ms = 10",
      'reference_channels = "mono"',
      "",
      "[[chain]]",
      'kind = "localvqe"',
      `model = ${tomlString(model)}`,
      `library = ${tomlString(library)}`,
      "threads = 1",
      "noise_gate = false",
      "",
    ].join("\n"),
  );

  const env = { ...process.env, ECHOLESS_LOCALVQE_LIBRARY: library };
  const pathKey = process.platform === "win32" ? "PATH" : "PATH";
  env[pathKey] = [nativeDir, env[pathKey]].filter(Boolean).join(path.delimiter);
  env.LD_LIBRARY_PATH = [nativeDir, env.LD_LIBRARY_PATH].filter(Boolean).join(path.delimiter);
  env.DYLD_LIBRARY_PATH = [nativeDir, env.DYLD_LIBRARY_PATH].filter(Boolean).join(path.delimiter);
  env.DYLD_FALLBACK_LIBRARY_PATH = [nativeDir, env.DYLD_FALLBACK_LIBRARY_PATH]
    .filter(Boolean)
    .join(path.delimiter);

  run(layout.cli, [
    "offline",
    "--config",
    configPath,
    "--mic",
    micPath,
    "--reference",
    refPath,
    "--out",
    outPath,
  ], { env });
  const outStat = fs.statSync(outPath);
  if (outStat.size <= 44) throw new Error(`LocalVQE output is empty: ${outPath}`);

  console.log(`bundle-smoke: localvqe output=${outPath}`);
  console.log("bundle-smoke: ok");
}

try {
  smoke();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
