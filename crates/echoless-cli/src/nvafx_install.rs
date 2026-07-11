use std::collections::HashMap;
use std::env;
use std::fs::{create_dir_all, remove_dir_all, rename, File};
use std::io::{copy, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Subcommand};
use serde_json::json;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use echoless_audio_io::file::{WavFileSink, WavFileSource};
use echoless_core::{
    default_output_level, run_offline, DiagnosticsConfig, PipelineConfig, ReferenceChannels,
};
use echoless_processors::NodeConfig;

const DEFAULT_NVAFX_RELEASE_TAG: &str = "rtx-aec-runtime-win64-2.1.0-aec48-preview.1";
const NVAFX_RELEASE_DOWNLOAD_BASE: &str = "https://github.com/Haor/echoless/releases/download";
const NVAFX_COMMON_RUNTIME_ASSET: &str = "echoless-rtx-aec-common-runtime-win64-2.1.0.zip";
const DOWNLOAD_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const DOWNLOAD_PROGRESS_STOP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
struct NvafxReleasePin {
    asset: &'static str,
    sha256: &'static str,
    size: u64,
}

// Trust anchor for DEFAULT_NVAFX_RELEASE_TAG. Release SHA256SUMS.txt is only a cross-check
// for these assets; custom tags may still use release-provided sums.
const NVAFX_DEFAULT_RELEASE_PINS: &[NvafxReleasePin] = &[
    NvafxReleasePin {
        asset: NVAFX_COMMON_RUNTIME_ASSET,
        sha256: "dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb",
        size: 1_001_010_819,
    },
    NvafxReleasePin {
        asset: "echoless-rtx-aec-model-win64-2.1.0-turing-aec48.zip",
        sha256: "951e03bb144156f4b27cbf2caa6930f9dabc3f1cb26a0afd9d9523f4d286dae9",
        size: 48_104_228,
    },
    NvafxReleasePin {
        asset: "echoless-rtx-aec-model-win64-2.1.0-ampere-aec48.zip",
        sha256: "066e06ec18a7d4509675411a1e050e11b0cfc4fee30d69d783871333018c9ab9",
        size: 48_049_936,
    },
    NvafxReleasePin {
        asset: "echoless-rtx-aec-model-win64-2.1.0-ada-aec48.zip",
        sha256: "92170e6a259f9093397b93cf4385759c36697ecb9e308322405bce1abcb8e3df",
        size: 48_374_718,
    },
    NvafxReleasePin {
        asset: "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip",
        sha256: "0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b",
        size: 48_927_764,
    },
];

#[derive(Args)]
pub(crate) struct NvafxArgs {
    #[command(subcommand)]
    cmd: NvafxCmd,
}

#[derive(Subcommand)]
enum NvafxCmd {
    /// Check RTX AEC runtime, GPU, driver, and VC++ runtime availability
    Doctor(NvafxDoctorArgs),
    /// Run RTX AEC offline: mic.wav + ref.wav -> out.wav
    Offline(NvafxOfflineArgs),
    /// Install Echoless RTX AEC runtime and model from a local zip
    Install(NvafxInstallArgs),
    /// Download and install RTX AEC runtime and current GPU model from the Echoless GitHub public release
    DownloadInstall(NvafxDownloadInstallArgs),
}

#[derive(Args)]
struct NvafxDoctorArgs {
    /// Emit JSON for GUI/installer consumers
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct NvafxOfflineArgs {
    /// Near-end microphone WAV
    #[arg(long)]
    mic: String,
    /// Far-end reference WAV
    #[arg(long)]
    reference: String,
    /// Output WAV
    #[arg(long)]
    out: String,
    /// Override model path; defaults are chosen automatically by GPU architecture
    #[arg(long)]
    model_path: Option<PathBuf>,
    /// AFX AEC intensity
    #[arg(long, default_value_t = 1.0)]
    intensity_ratio: f32,
}

#[derive(Args)]
struct NvafxInstallArgs {
    /// common runtime zip
    #[arg(long)]
    common_zip: PathBuf,
    /// Model zip for the current GPU architecture
    #[arg(long)]
    model_zip: PathBuf,
    /// Override expected SHA256 for common zip; falls back to matching the official asset name when unset
    #[arg(long)]
    common_sha256: Option<String>,
    /// Override expected SHA256 for model zip; falls back to matching the official asset name when unset
    #[arg(long)]
    model_sha256: Option<String>,
}

#[derive(Args)]
struct NvafxDownloadInstallArgs {
    /// GitHub release tag; defaults to the Echoless RTX AEC public preview release
    #[arg(long, default_value = DEFAULT_NVAFX_RELEASE_TAG)]
    tag: String,
    /// Emit { ok, report } JSON for GUI installer consumers
    #[arg(long)]
    json: bool,
}

pub(crate) fn cmd_nvafx(args: NvafxArgs) -> Result<()> {
    match args.cmd {
        NvafxCmd::Doctor(a) => cmd_nvafx_doctor(a),
        NvafxCmd::Offline(a) => cmd_nvafx_offline(a),
        NvafxCmd::Install(a) => cmd_nvafx_install(a),
        NvafxCmd::DownloadInstall(a) => cmd_nvafx_download_install(a),
    }
}

fn cmd_nvafx_doctor(args: NvafxDoctorArgs) -> Result<()> {
    let report = echoless_processors::nvafx::doctor_report()?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": report.ok(),
                "report": report,
            }))?
        );
        return Ok(());
    }

    println!("NVIDIA AFX / RTX AEC doctor");
    println!(
        "SDK {} · runtime file {} · minimum driver {}",
        echoless_processors::nvafx::SDK_VERSION,
        echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        echoless_processors::nvafx::MIN_DRIVER_VERSION,
    );
    println!(
        "Runtime: {} ({})",
        report.runtime_dir.display(),
        report.runtime_dir_source
    );
    if report.gpus.is_empty() {
        println!("GPU:     no NVIDIA GPU detected");
    } else {
        println!("GPU:");
        for (index, gpu) in report.gpus.iter().enumerate() {
            let arch = gpu
                .arch
                .map(|arch| arch.as_str().to_string())
                .unwrap_or_else(|| "unsupported".to_string());
            println!(
                "  [{index}] {} · driver {} · compute_cap {} · arch {}",
                gpu.name, gpu.driver_version, gpu.compute_capability, arch
            );
        }
    }
    if let Some(asset) = report.expected_model_asset() {
        println!("Model asset: {asset}");
    }
    println!();

    let mut problems = 0usize;
    for check in &report.checks {
        if check.status.is_problem() {
            problems += 1;
        }
        println!(
            "[{}] {} — {}",
            check.status.label(),
            check.name,
            check.detail
        );
        if let Some(action) = &check.action {
            println!("      action: {action}");
        }
    }

    if problems == 0 {
        println!("\nRTX AEC runtime preflight passed.");
    } else {
        println!("\nRTX AEC runtime preflight failed: {problems} issues to resolve.");
    }
    Ok(())
}

fn cmd_nvafx_offline(a: NvafxOfflineArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx offline")?;
    if !a.intensity_ratio.is_finite() || a.intensity_ratio < 0.0 {
        bail!("--intensity-ratio must be a non-negative finite number");
    }
    let mut params = toml::Table::new();
    if let Some(model_path) = &a.model_path {
        params.insert(
            "model_path".into(),
            toml::Value::String(model_path.display().to_string()),
        );
    }
    params.insert(
        "intensity_ratio".into(),
        toml::Value::Float(a.intensity_ratio as f64),
    );

    let cfg = PipelineConfig {
        mic: a.mic.clone(),
        reference: a.reference.clone(),
        output: a.out.clone(),
        sample_rate: echoless_processors::nvafx::NVAFX_SAMPLE_RATE,
        frame_ms: 10,
        reference_channels: ReferenceChannels::Mono,
        near_delay_ms: 0,
        output_level: default_output_level(),
        bypass: false,
        output_rate_match: echoless_core::default_output_rate_match(),
        diagnostics: DiagnosticsConfig::default(),
        chain: vec![NodeConfig {
            kind: "nvidia_afx_aec".into(),
            params,
        }],
    };
    validate_nvafx_constraints(&cfg)?;

    let frame = cfg.frame_size();
    let mic = WavFileSource::new(&a.mic, frame)?;
    let reference = WavFileSource::new(&a.reference, frame)?;
    let sink = WavFileSink::new(&a.out);
    println!(
        "RTX AEC offline run: {} + {} -> {}",
        a.mic, a.reference, a.out
    );
    let rep = run_offline(&cfg, mic, reference, sink)?;
    println!(
        "done: {} frames (~{:.2}s) · process chain [{}]",
        rep.frames,
        rep.seconds,
        rep.chain.join(", ")
    );
    for s in &rep.node_stats {
        println!(
            "  - {}: process {:.2} ms, runtime_errors={}, diverged={}",
            s.name, s.process_time_ms, s.runtime_error_count, s.diverged
        );
        if let Some(arch) = &s.selected_gpu_arch {
            println!("      arch={arch}");
        }
        if let Some(model) = &s.selected_model {
            println!("      model={model}");
        }
        if let Some(err) = &s.last_backend_error {
            println!("      last_error={err}");
        }
    }
    Ok(())
}

fn cmd_nvafx_install(a: NvafxInstallArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx install")?;
    let report = install_nvafx_runtime(NvafxInstallRequest {
        common_zip: &a.common_zip,
        model_zip: &a.model_zip,
        common_sha256: a.common_sha256.as_deref(),
        model_sha256: a.model_sha256.as_deref(),
        install_source: json!({ "kind": "local-zip" }),
        log_to_stderr: false,
    })?;
    print_nvafx_doctor_report(&report);
    if !report.ok() {
        bail!("runtime extracted, but doctor still did not pass");
    }
    Ok(())
}

fn cmd_nvafx_download_install(a: NvafxDownloadInstallArgs) -> Result<()> {
    ensure_nvafx_windows_command("nvafx download-install")?;
    let tag = a.tag.trim();
    if tag.is_empty() {
        bail!("--tag must not be empty");
    }

    let preflight = echoless_processors::nvafx::doctor_report()?;
    let arch = preflight.selected_arch.with_context(|| {
        "unable to determine GPU architecture from nvafx doctor; verify nvidia-smi, driver, and RTX GPU availability first"
    })?;
    let model_asset = arch.model_asset_name();
    let download_dir = nvafx_download_cache_dir(tag);
    create_dir_all(&download_dir).with_context(|| {
        format!(
            "failed to create download cache directory: {}",
            download_dir.display()
        )
    })?;

    install_log(
        a.json,
        format!(
            "RTX AEC download source: GitHub release {tag} · arch={}",
            arch.as_str()
        ),
    );

    let release_sha256sums = match fetch_release_sha256sums(tag, &download_dir, a.json) {
        Ok(sums) => sums,
        Err(err) => {
            install_log(
                a.json,
                format!("failed to read SHA256SUMS.txt: {err:#}; will fall back to built-in hashes or only record actual hashes"),
            );
            HashMap::new()
        }
    };

    let common_zip = download_dir.join(NVAFX_COMMON_RUNTIME_ASSET);
    let model_zip = download_dir.join(&model_asset);
    let common_url = nvafx_release_asset_url(tag, NVAFX_COMMON_RUNTIME_ASSET);
    let model_url = nvafx_release_asset_url(tag, &model_asset);
    let common_expected =
        expected_sha256_for_release_asset(tag, &release_sha256sums, NVAFX_COMMON_RUNTIME_ASSET)?;
    let model_expected = expected_sha256_for_release_asset(tag, &release_sha256sums, &model_asset)?;
    let common_size = expected_size_for_release_asset(tag, NVAFX_COMMON_RUNTIME_ASSET);
    let model_size = expected_size_for_release_asset(tag, &model_asset);

    download_release_asset(
        &common_url,
        &common_zip,
        common_expected.as_deref(),
        common_size,
        "common runtime",
        a.json,
    )?;
    download_release_asset(
        &model_url,
        &model_zip,
        model_expected.as_deref(),
        model_size,
        "model",
        a.json,
    )?;

    let report = install_nvafx_runtime(NvafxInstallRequest {
        common_zip: &common_zip,
        model_zip: &model_zip,
        common_sha256: common_expected.as_deref(),
        model_sha256: model_expected.as_deref(),
        install_source: json!({
            "kind": "github-release",
            "tag": tag,
            "arch": arch.as_str(),
            "common_url": common_url,
            "model_url": model_url,
        }),
        log_to_stderr: a.json,
    })?;

    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": report.ok(),
                "report": report,
            }))?
        );
    } else {
        print_nvafx_doctor_report(&report);
    }
    if !report.ok() {
        // doctor 未过:保留下载缓存,便于用户排查后重装而无需重下 ~1 GB。
        bail!("runtime downloaded and extracted, but doctor still did not pass");
    }
    // 安装成功且 doctor 通过 —— common runtime + model 已完全解压到固定 runtime 目录,
    // TMP 里的下载缓存(~1 GB)不再需要,自动清掉。清理失败不影响安装结果,仅记日志。
    match remove_dir_all(&download_dir) {
        Ok(()) => install_log(
            a.json,
            format!("cleaned up download cache: {}", download_dir.display()),
        ),
        Err(err) => install_log(
            a.json,
            format!(
                "failed to clean up download cache (ignorable, remove manually): {}: {err:#}",
                download_dir.display()
            ),
        ),
    }
    Ok(())
}

struct NvafxInstallRequest<'a> {
    common_zip: &'a Path,
    model_zip: &'a Path,
    common_sha256: Option<&'a str>,
    model_sha256: Option<&'a str>,
    install_source: serde_json::Value,
    log_to_stderr: bool,
}

fn install_nvafx_runtime(
    request: NvafxInstallRequest<'_>,
) -> Result<echoless_processors::nvafx::DoctorReport> {
    let (runtime_dir, runtime_dir_source) = echoless_processors::nvafx::resolve_runtime_dir();
    if let Some(parent) = runtime_dir.parent() {
        create_dir_all(parent).with_context(|| {
            format!(
                "failed to create runtime parent directory: {}",
                parent.display()
            )
        })?;
    }

    let common_expected = request
        .common_sha256
        .or_else(|| expected_sha256_for_asset(request.common_zip));
    let model_expected = request
        .model_sha256
        .or_else(|| expected_sha256_for_asset(request.model_zip));
    let common_hash = verify_zip_sha256(
        request.common_zip,
        common_expected,
        "common runtime",
        request.log_to_stderr,
    )?;
    let model_hash = verify_zip_sha256(
        request.model_zip,
        model_expected,
        "model",
        request.log_to_stderr,
    )?;

    install_log(
        request.log_to_stderr,
        format!(
            "extracting common runtime to staging, then switching to {}",
            runtime_dir.display()
        ),
    );
    let staging_dir = unique_install_staging_dir(&runtime_dir)?;
    extract_zip(request.common_zip, &staging_dir)?;
    install_log(
        request.log_to_stderr,
        format!("extracting model to staging: {}", staging_dir.display()),
    );
    extract_zip(request.model_zip, &staging_dir)?;

    let installed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before UNIX_EPOCH")?
        .as_secs();
    let manifest = json!({
        "sdk_version": echoless_processors::nvafx::SDK_VERSION,
        "runtime_file_version": echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        "installed_at_unix": installed_at,
        "runtime_dir_source": runtime_dir_source,
        "common_zip": request.common_zip.display().to_string(),
        "common_sha256": common_hash,
        "model_zip": request.model_zip.display().to_string(),
        "model_sha256": model_hash,
        "install_source": request.install_source,
    });
    let manifest_path = staging_dir.join("echoless-runtime-install-manifest.json");
    let mut file = File::create(&manifest_path).with_context(|| {
        format!(
            "failed to write install manifest: {}",
            manifest_path.display()
        )
    })?;
    file.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    file.write_all(b"\n")?;
    drop(file);

    replace_dir_with_staging(&runtime_dir, &staging_dir)?;

    install_log(
        request.log_to_stderr,
        format!(
            "install manifest: {}",
            runtime_dir
                .join("echoless-runtime-install-manifest.json")
                .display()
        ),
    );
    echoless_processors::nvafx::doctor_report()
}

fn ensure_nvafx_windows_command(command: &str) -> Result<()> {
    if !cfg!(windows) {
        bail!("{command} currently supports Windows x64 only; macOS artifacts can only use the AEC3/LocalVQE path");
    }
    Ok(())
}

fn install_log(log_to_stderr: bool, message: impl AsRef<str>) {
    if log_to_stderr {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

fn nvafx_download_cache_dir(tag: &str) -> PathBuf {
    env::temp_dir()
        .join("echoless-nvafx-download")
        .join(sanitize_release_tag(tag))
}

fn sanitize_release_tag(tag: &str) -> String {
    let sanitized = tag
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "release".to_string()
    } else {
        sanitized
    }
}

fn nvafx_release_asset_url(tag: &str, asset: &str) -> String {
    format!(
        "{}/{}/{}",
        NVAFX_RELEASE_DOWNLOAD_BASE,
        encode_url_path_segment(tag),
        encode_url_path_segment(asset)
    )
}

fn encode_url_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn fetch_release_sha256sums(
    tag: &str,
    download_dir: &Path,
    log_to_stderr: bool,
) -> Result<HashMap<String, String>> {
    let path = download_dir.join("SHA256SUMS.txt");
    let url = nvafx_release_asset_url(tag, "SHA256SUMS.txt");
    download_file(&url, &path, "SHA256SUMS.txt", None, log_to_stderr)?;
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read SHA256SUMS.txt: {}", path.display()))?;
    Ok(parse_sha256sums(&contents))
}

fn parse_sha256sums(contents: &str) -> HashMap<String, String> {
    let mut sums = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(asset) = parts.next() else {
            continue;
        };
        if hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            sums.insert(
                asset.trim_start_matches('*').to_string(),
                hash.to_ascii_lowercase(),
            );
        }
    }
    sums
}

fn expected_sha256_for_release_asset(
    tag: &str,
    release_sha256sums: &HashMap<String, String>,
    asset: &str,
) -> Result<Option<String>> {
    if tag == DEFAULT_NVAFX_RELEASE_TAG {
        if let Some(embedded) = expected_sha256_for_asset(Path::new(asset)) {
            if let Some(release) = release_sha256sums.get(asset) {
                ensure!(
                    release.eq_ignore_ascii_case(embedded),
                    "SHA256SUMS.txt does not match the built-in pin: asset={asset}, release={release}, embedded={embedded}"
                );
            }
            return Ok(Some(embedded.to_string()));
        }
    }

    Ok(release_sha256sums.get(asset).cloned())
}

fn expected_size_for_release_asset(tag: &str, asset: &str) -> Option<u64> {
    (tag == DEFAULT_NVAFX_RELEASE_TAG)
        .then(|| default_release_pin(asset).map(|pin| pin.size))
        .flatten()
}

fn download_release_asset(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    expected_size: Option<u64>,
    label: &str,
    log_to_stderr: bool,
) -> Result<()> {
    if dest.exists() {
        match expected_sha256 {
            Some(expected) => {
                let actual = sha256_file(dest).with_context(|| {
                    format!("failed to verify existing download: {}", dest.display())
                })?;
                if actual.eq_ignore_ascii_case(expected) {
                    install_log(
                        log_to_stderr,
                        format!("{label} already cached and SHA256 ok: {}", dest.display()),
                    );
                    return Ok(());
                }
                install_log(
                    log_to_stderr,
                    format!(
                        "{label} cached SHA256 mismatch; re-downloading: {}",
                        dest.display()
                    ),
                );
            }
            None => {
                install_log(
                    log_to_stderr,
                    format!(
                        "{label} already cached, no expected SHA256 provided; re-downloading: {}",
                        dest.display()
                    ),
                );
            }
        }
    }
    download_file(url, dest, label, expected_size, log_to_stderr)
}

fn download_file(
    url: &str,
    dest: &Path,
    label: &str,
    expected_size: Option<u64>,
    log_to_stderr: bool,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        create_dir_all(parent).with_context(|| {
            format!("failed to create download directory: {}", parent.display())
        })?;
    }
    let tmp = dest.with_extension(format!(
        "{}part",
        dest.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!("{ext}."))
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_file(&tmp);
    install_log(log_to_stderr, format!("downloading {label}: {url}"));
    let progress = log_to_stderr.then(|| {
        spawn_download_progress_observer(
            tmp.clone(),
            label.to_string(),
            expected_size,
            DOWNLOAD_PROGRESS_INTERVAL,
            DOWNLOAD_PROGRESS_STOP_TIMEOUT,
            |label, received, total| emit_download_progress(true, label, received, total),
        )
    });

    match download_with_powershell(url, &tmp) {
        Ok(()) => {
            let _ = std::fs::remove_file(dest);
            rename(&tmp, dest).with_context(|| {
                format!(
                    "failed to commit downloaded file: {} -> {}",
                    tmp.display(),
                    dest.display()
                )
            })
        }
        Err(power_shell_err) => {
            let _ = std::fs::remove_file(&tmp);
            install_log(
                log_to_stderr,
                format!("PowerShell download failed, trying curl.exe: {power_shell_err:#}"),
            );
            download_with_curl(url, &tmp)
                .with_context(|| format!("PowerShell download also failed: {power_shell_err:#}"))?;
            let _ = std::fs::remove_file(dest);
            rename(&tmp, dest).with_context(|| {
                format!(
                    "failed to commit downloaded file: {} -> {}",
                    tmp.display(),
                    dest.display()
                )
            })
        }
    }?;

    if let Some(observer) = progress {
        observer.stop();
    }
    let received = std::fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    emit_download_progress(log_to_stderr, label, received, expected_size);
    Ok(())
}

struct DownloadProgressObserver {
    stop: Option<Sender<()>>,
    done: Receiver<()>,
    handle: Option<JoinHandle<()>>,
    stop_timeout: Duration,
}

impl DownloadProgressObserver {
    fn stop(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if self.handle.is_none() {
            return;
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.done.recv_timeout(self.stop_timeout) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Progress is optional: detach a stuck observer instead of delaying
                // hashing, installation, or process exit.
                self.handle.take();
            }
        }
    }
}

impl Drop for DownloadProgressObserver {
    fn drop(&mut self) {
        self.finish();
    }
}

fn spawn_download_progress_observer<F>(
    path: PathBuf,
    label: String,
    total: Option<u64>,
    interval: Duration,
    stop_timeout: Duration,
    emit: F,
) -> DownloadProgressObserver
where
    F: Fn(&str, u64, Option<u64>) + Send + 'static,
{
    let (stop_tx, stop_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut last_received = None;
        loop {
            let received = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if last_received != Some(received) {
                emit(&label, received, total);
                last_received = Some(received);
            }
            match stop_rx.recv_timeout(interval) {
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = done_tx.send(());
    });
    DownloadProgressObserver {
        stop: Some(stop_tx),
        done: done_rx,
        handle: Some(handle),
        stop_timeout,
    }
}

fn emit_download_progress(enabled: bool, label: &str, received: u64, total: Option<u64>) {
    if enabled {
        eprintln!("{}", download_progress_payload(label, received, total));
    }
}

fn download_progress_payload(label: &str, received: u64, total: Option<u64>) -> serde_json::Value {
    let total = total.filter(|value| *value > 0).unwrap_or(0);
    let pct = (total > 0).then(|| {
        received
            .min(total)
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0) as u32
    });
    json!({
        "event": "nvafx_download_progress",
        "label": label,
        "received": received,
        "total": total,
        "pct": pct,
    })
}

fn download_with_powershell(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("powershell.exe")
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg("$ProgressPreference = 'SilentlyContinue'; Invoke-WebRequest -Uri $args[0] -OutFile $args[1] -UseBasicParsing")
        .arg(url)
        .arg(dest)
        .output()
        .context("failed to launch powershell.exe")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "powershell.exe exit={}; stderr={}; stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    )
}

fn download_with_curl(url: &str, dest: &Path) -> Result<()> {
    let output = Command::new("curl.exe")
        .arg("-L")
        .arg("--fail")
        .arg("--retry")
        .arg("2")
        .arg("--output")
        .arg(dest)
        .arg(url)
        .output()
        .context("failed to launch curl.exe")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "curl.exe exit={}; stderr={}; stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    )
}

fn print_nvafx_doctor_report(report: &echoless_processors::nvafx::DoctorReport) {
    println!("NVIDIA AFX / RTX AEC doctor");
    println!(
        "SDK {} · runtime file {} · minimum driver {}",
        echoless_processors::nvafx::SDK_VERSION,
        echoless_processors::nvafx::RUNTIME_FILE_VERSION,
        echoless_processors::nvafx::MIN_DRIVER_VERSION,
    );
    println!(
        "Runtime: {} ({})",
        report.runtime_dir.display(),
        report.runtime_dir_source
    );
    if report.gpus.is_empty() {
        println!("GPU:     no NVIDIA GPU detected");
    } else {
        println!("GPU:");
        for (index, gpu) in report.gpus.iter().enumerate() {
            let arch = gpu
                .arch
                .map(|arch| arch.as_str().to_string())
                .unwrap_or_else(|| "unsupported".to_string());
            println!(
                "  [{index}] {} · driver {} · compute_cap {} · arch {}",
                gpu.name, gpu.driver_version, gpu.compute_capability, arch
            );
        }
    }
    if let Some(asset) = report.expected_model_asset() {
        println!("Model asset: {asset}");
    }
    println!();

    let mut problems = 0usize;
    for check in &report.checks {
        if check.status.is_problem() {
            problems += 1;
        }
        println!(
            "[{}] {} — {}",
            check.status.label(),
            check.name,
            check.detail
        );
        if let Some(action) = &check.action {
            println!("      action: {action}");
        }
    }

    if problems == 0 {
        println!("\nRTX AEC runtime preflight passed.");
    } else {
        println!("\nRTX AEC runtime preflight failed: {problems} issues to resolve.");
    }
}

fn expected_sha256_for_asset(path: &Path) -> Option<&'static str> {
    let asset = path.file_name()?.to_str()?;
    default_release_pin(asset).map(|pin| pin.sha256)
}

fn default_release_pin(asset: &str) -> Option<&'static NvafxReleasePin> {
    NVAFX_DEFAULT_RELEASE_PINS
        .iter()
        .find(|pin| pin.asset == asset)
}

fn verify_zip_sha256(
    path: &Path,
    expected: Option<&str>,
    label: &str,
    log_to_stderr: bool,
) -> Result<String> {
    let actual = sha256_file(path)?;
    match expected {
        Some(expected) if actual.eq_ignore_ascii_case(expected) => {
            install_log(log_to_stderr, format!("{label} SHA256 ok: {actual}"));
        }
        Some(expected) => bail!(
            "{label} SHA256 mismatch: actual={actual}, expected={expected}, file={}",
            path.display()
        ),
        None => {
            install_log(
                log_to_stderr,
                format!(
                    "{label} SHA256: {actual} (no official expected value found, recording only; consider passing --{}-sha256)",
                    if label.starts_with("common") {
                        "common"
                    } else {
                        "model"
                    }
                ),
            );
        }
    }
    Ok(actual)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open file: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("failed to read file: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(zip_path)
        .with_context(|| format!("failed to open zip: {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("failed to read zip: {}", zip_path.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).with_context(|| {
            format!("failed to read zip entry #{index}: {}", zip_path.display())
        })?;
        let enclosed = entry
            .enclosed_name()
            .with_context(|| format!("unsafe zip entry path: {}", entry.name()))?;
        let out_path = dest.join(enclosed);
        if entry.is_dir() {
            create_dir_all(&out_path)
                .with_context(|| format!("failed to create directory: {}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            create_dir_all(parent)
                .with_context(|| format!("failed to create directory: {}", parent.display()))?;
        }
        let mut out = File::create(&out_path)
            .with_context(|| format!("failed to create file: {}", out_path.display()))?;
        copy(&mut entry, &mut out)
            .with_context(|| format!("failed to extract file: {}", out_path.display()))?;
    }
    Ok(())
}

fn unique_install_staging_dir(runtime_dir: &Path) -> Result<PathBuf> {
    let parent = runtime_dir
        .parent()
        .with_context(|| format!("runtime directory has no parent: {}", runtime_dir.display()))?;
    let name = runtime_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nvafx-runtime");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before UNIX_EPOCH")?
        .as_nanos();
    let staging = parent.join(format!("{name}.installing-{}-{nanos}", std::process::id()));
    if staging.exists() {
        remove_dir_all(&staging).with_context(|| {
            format!(
                "failed to clean up old staging directory: {}",
                staging.display()
            )
        })?;
    }
    create_dir_all(&staging)
        .with_context(|| format!("failed to create staging directory: {}", staging.display()))?;
    Ok(staging)
}

fn replace_dir_with_staging(runtime_dir: &Path, staging_dir: &Path) -> Result<()> {
    let backup_dir = runtime_dir.with_extension(format!(
        "previous-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time is before UNIX_EPOCH")?
            .as_nanos()
    ));
    let had_previous = runtime_dir.exists();
    if had_previous {
        rename(runtime_dir, &backup_dir).with_context(|| {
            format!(
                "failed to move old runtime directory: {} -> {}",
                runtime_dir.display(),
                backup_dir.display()
            )
        })?;
    }
    if let Err(err) = rename(staging_dir, runtime_dir) {
        if had_previous {
            let _ = rename(&backup_dir, runtime_dir);
        }
        bail!(
            "failed to commit runtime staging directory: {} -> {}: {err}",
            staging_dir.display(),
            runtime_dir.display()
        );
    }
    if had_previous {
        let _ = remove_dir_all(backup_dir);
    }
    Ok(())
}

pub(crate) fn validate_nvafx_constraints(cfg: &PipelineConfig) -> Result<()> {
    if !cfg.chain.iter().any(|node| node.kind == "nvidia_afx_aec") {
        return Ok(());
    }
    if cfg.sample_rate != echoless_processors::nvafx::NVAFX_SAMPLE_RATE {
        bail!(
            "nvidia_afx_aec v1 only supports {} Hz; current sample_rate={}",
            echoless_processors::nvafx::NVAFX_SAMPLE_RATE,
            cfg.sample_rate
        );
    }
    if cfg.frame_ms != 10 {
        bail!(
            "nvidia_afx_aec v1 only supports 10ms frames; current frame_ms={}",
            cfg.frame_ms
        );
    }
    if cfg.reference_channels != ReferenceChannels::Mono {
        bail!("nvidia_afx_aec v1 only supports mono reference; set reference_channels = \"mono\"");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvafx_release_asset_url_uses_public_release_base() {
        let url = nvafx_release_asset_url(
            DEFAULT_NVAFX_RELEASE_TAG,
            "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip",
        );

        assert_eq!(
            url,
            "https://github.com/Haor/echoless/releases/download/rtx-aec-runtime-win64-2.1.0-aec48-preview.1/echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"
        );
    }

    #[test]
    fn nvafx_release_asset_url_encodes_tag_path_segments() {
        let url = nvafx_release_asset_url("preview/a b", "asset.zip");

        assert_eq!(
            url,
            "https://github.com/Haor/echoless/releases/download/preview%2Fa%20b/asset.zip"
        );
    }

    #[test]
    fn nvafx_download_progress_reports_known_asset_percentage() {
        let total = 1_001_010_819;
        let start = download_progress_payload("common runtime", 0, Some(total));
        assert_eq!(start["event"], "nvafx_download_progress");
        assert_eq!(start["label"], "common runtime");
        assert_eq!(start["received"], 0);
        assert_eq!(start["total"], total);
        assert_eq!(start["pct"], 0);

        let halfway = download_progress_payload("common runtime", total / 2, Some(total));
        assert_eq!(halfway["pct"], 49);

        let complete = download_progress_payload("common runtime", total, Some(total));
        assert_eq!(complete["received"], total);
        assert_eq!(complete["total"], total);
        assert_eq!(complete["pct"], 100);

        let custom = download_progress_payload("custom asset", 12_345, None);
        assert_eq!(custom["received"], 12_345);
        assert_eq!(custom["total"], 0);
        assert!(custom["pct"].is_null());
    }

    #[test]
    fn official_nvafx_release_assets_have_pinned_sizes() {
        assert_eq!(
            expected_size_for_release_asset(DEFAULT_NVAFX_RELEASE_TAG, NVAFX_COMMON_RUNTIME_ASSET),
            Some(1_001_010_819)
        );
        assert_eq!(
            expected_size_for_release_asset(
                DEFAULT_NVAFX_RELEASE_TAG,
                "echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"
            ),
            Some(48_927_764)
        );
        assert_eq!(
            expected_size_for_release_asset("custom-tag", NVAFX_COMMON_RUNTIME_ASSET),
            None
        );
    }

    #[test]
    fn local_progress_observer_reports_growing_partial_file() {
        let path = env::temp_dir().join(format!(
            "echoless-nvafx-progress-{}-{}.part",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, []).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let observer = spawn_download_progress_observer(
            path.clone(),
            "common runtime".to_string(),
            Some(100),
            Duration::from_millis(5),
            Duration::from_millis(50),
            move |label, received, total| {
                let _ = tx.send(download_progress_payload(label, received, total));
            },
        );

        let start = rx.recv_timeout(Duration::from_millis(250)).unwrap();
        assert_eq!(start["received"], 0);
        assert_eq!(start["pct"], 0);

        std::fs::write(&path, vec![0u8; 50]).unwrap();
        let halfway = loop {
            let payload = rx.recv_timeout(Duration::from_millis(250)).unwrap();
            if payload["received"] == 50 {
                break payload;
            }
        };
        assert_eq!(halfway["pct"], 50);

        std::fs::write(&path, vec![0u8; 100]).unwrap();
        let complete = loop {
            let payload = rx.recv_timeout(Duration::from_millis(250)).unwrap();
            if payload["received"] == 100 {
                break payload;
            }
        };
        assert_eq!(complete["pct"], 100);

        observer.stop();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn progress_observer_stop_is_bounded_when_emission_stalls() {
        let path = env::temp_dir().join(format!(
            "echoless-nvafx-progress-stall-{}-{}.part",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, []).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let observer = spawn_download_progress_observer(
            path.clone(),
            "common runtime".to_string(),
            Some(100),
            Duration::from_millis(5),
            Duration::from_millis(20),
            move |_, _, _| {
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
            },
        );
        entered_rx.recv_timeout(Duration::from_millis(250)).unwrap();

        let started = std::time::Instant::now();
        observer.stop();
        assert!(started.elapsed() < Duration::from_millis(250));

        let _ = release_tx.send(());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn parse_sha256sums_accepts_common_formats() {
        let sums = parse_sha256sums(
            r#"
            # comment
            DCACAC954B7973AE18369B252D13F24B973B10114D00E5293EAB0713601C7BCB  echoless-rtx-aec-common-runtime-win64-2.1.0.zip
            0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b *echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip
            invalid line
            "#,
        );

        assert_eq!(
            sums["echoless-rtx-aec-common-runtime-win64-2.1.0.zip"],
            "dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb"
        );
        assert_eq!(
            sums["echoless-rtx-aec-model-win64-2.1.0-blackwell-aec48.zip"],
            "0e75bb7442d317990ef0d5a6477105f86b9bbae1c2c5e4a6bdfb8d4e9f42df5b"
        );
    }

    #[test]
    fn embedded_nvafx_release_pins_are_well_formed() {
        let mut assets = std::collections::HashSet::new();
        for pin in NVAFX_DEFAULT_RELEASE_PINS {
            assert!(
                assets.insert(pin.asset),
                "duplicate asset pin: {}",
                pin.asset
            );
            assert_eq!(pin.sha256.len(), 64, "bad hash length: {}", pin.asset);
            assert!(
                pin.sha256.chars().all(|ch| ch.is_ascii_hexdigit()),
                "bad hash characters: {}",
                pin.asset
            );
            assert_eq!(
                expected_sha256_for_asset(Path::new(pin.asset)),
                Some(pin.sha256)
            );
        }
        assert!(assets.contains(NVAFX_COMMON_RUNTIME_ASSET));
    }

    #[test]
    fn expected_sha256_prefers_default_embedded_values_and_rejects_mismatch() {
        let mut sums = HashMap::new();
        sums.insert(
            NVAFX_COMMON_RUNTIME_ASSET.to_string(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        );

        let err = expected_sha256_for_release_asset(
            DEFAULT_NVAFX_RELEASE_TAG,
            &sums,
            NVAFX_COMMON_RUNTIME_ASSET,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("SHA256SUMS.txt does not match the built-in pin"),
            "{err}"
        );

        sums.insert(
            NVAFX_COMMON_RUNTIME_ASSET.to_string(),
            "dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb".to_string(),
        );
        assert_eq!(
            expected_sha256_for_release_asset(
                DEFAULT_NVAFX_RELEASE_TAG,
                &sums,
                NVAFX_COMMON_RUNTIME_ASSET
            )
            .unwrap()
            .as_deref(),
            Some("dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb")
        );
        assert_eq!(
            expected_sha256_for_release_asset(
                DEFAULT_NVAFX_RELEASE_TAG,
                &HashMap::new(),
                NVAFX_COMMON_RUNTIME_ASSET,
            )
            .unwrap()
            .as_deref(),
            Some("dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb")
        );
        assert_eq!(
            expected_sha256_for_release_asset(
                "custom-tag",
                &HashMap::new(),
                NVAFX_COMMON_RUNTIME_ASSET
            )
            .unwrap(),
            None
        );

        assert_eq!(
            expected_sha256_for_release_asset("custom-tag", &sums, NVAFX_COMMON_RUNTIME_ASSET)
                .unwrap()
                .as_deref(),
            Some("dcacac954b7973ae18369b252d13f24b973b10114d00e5293eab0713601c7bcb")
        );
    }

    #[test]
    fn nvafx_sha256_file_streams_large_files() {
        let path = env::temp_dir().join(format!(
            "echoless-nvafx-sha256-{}-{}.bin",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = vec![0x5au8; 2 * 1024 * 1024 + 17];
        std::fs::write(&path, &data).unwrap();

        let actual = sha256_file(&path).unwrap();
        let expected = format!("{:x}", Sha256::digest(&data));
        assert_eq!(actual, expected);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replace_dir_with_staging_swaps_runtime_tree() {
        let root = env::temp_dir().join(format!(
            "echoless-nvafx-staging-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let runtime = root.join("runtime");
        let sentinel = root.join("outside-sentinel.txt");
        let staging = unique_install_staging_dir(&runtime).unwrap();
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("old.txt"), b"old").unwrap();
        std::fs::write(staging.join("new.txt"), b"new").unwrap();
        std::fs::write(&sentinel, b"keep").unwrap();

        replace_dir_with_staging(&runtime, &staging).unwrap();

        assert!(!staging.exists());
        assert!(!runtime.join("old.txt").exists());
        assert_eq!(std::fs::read(runtime.join("new.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn replace_dir_with_staging_rolls_back_without_touching_sibling() {
        let root = env::temp_dir().join(format!(
            "echoless-nvafx-rollback-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let runtime = root.join("runtime");
        let sentinel = root.join("outside-sentinel.txt");
        let missing_staging = root.join("missing-staging");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("old.txt"), b"old").unwrap();
        std::fs::write(&sentinel, b"keep").unwrap();

        let error = replace_dir_with_staging(&runtime, &missing_staging).unwrap_err();

        assert!(error
            .to_string()
            .contains("failed to commit runtime staging"));
        assert_eq!(std::fs::read(runtime.join("old.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");

        let _ = std::fs::remove_dir_all(root);
    }
}
