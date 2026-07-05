use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{
    Device, DeviceDescription, SampleFormat, SupportedStreamConfig, SupportedStreamConfigRange,
};
use serde_json::{json, Value};

#[cfg(target_os = "macos")]
use super::macos_process_tap;

#[derive(Clone, Copy)]
pub(super) enum DeviceKind {
    Input,
    Output,
}
impl DeviceKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

pub(super) struct SelectedDevice {
    pub(super) index: Option<usize>,
    pub(super) device: Device,
}

pub(super) struct StreamConfigChoice {
    supported: SupportedStreamConfig,
    pub(super) pipeline_sample_rate: u32,
}

impl StreamConfigChoice {
    fn new(supported: SupportedStreamConfig, pipeline_sample_rate: u32) -> Self {
        Self {
            supported,
            pipeline_sample_rate,
        }
    }

    pub(super) fn stream_sample_rate(&self) -> u32 {
        self.supported.sample_rate()
    }

    pub(super) fn requires_resampling(&self) -> bool {
        self.stream_sample_rate() != self.pipeline_sample_rate
    }

    pub(super) fn channels(&self) -> u16 {
        self.supported.channels()
    }

    pub(super) fn sample_format(&self) -> SampleFormat {
        self.supported.sample_format()
    }

    pub(super) fn config(&self) -> cpal::StreamConfig {
        self.supported.config()
    }
}

pub(super) enum ReferenceSource {
    None,
    Cpal {
        device: SelectedDevice,
        kind: DeviceKind,
    },
    #[cfg(target_os = "macos")]
    ProcessTap,
}

impl ReferenceSource {
    pub(super) fn has_reference(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub(super) fn status_name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cpal { kind, .. } => match kind {
                DeviceKind::Input => "cpal_input",
                DeviceKind::Output => "cpal_output",
            },
            #[cfg(target_os = "macos")]
            Self::ProcessTap => "macos_process_tap",
        }
    }
}

// ── 设备选择 ──────────────────────────────────────────────────────────────────

pub(super) fn mic_selector(s: &str) -> Option<&str> {
    match s {
        "default" | "" => None,
        other => Some(other),
    }
}
pub(super) fn output_selector(s: &str) -> Option<&str> {
    match s {
        "default" | "" => None,
        other => Some(other),
    }
}

fn select_default_device(host: &cpal::Host, kind: DeviceKind) -> Result<SelectedDevice> {
    let device = match kind {
        DeviceKind::Input => host.default_input_device(),
        DeviceKind::Output => host.default_output_device(),
    }
    .with_context(|| format!("无默认 {} 设备", kind.label()))?;
    let devices = devices_for(host, kind).unwrap_or_default();
    let index = find_device_index(&devices, &device);
    Ok(SelectedDevice { index, device })
}

pub(super) fn select_device(
    host: &cpal::Host,
    kind: DeviceKind,
    selector: Option<&str>,
) -> Result<SelectedDevice> {
    if let Some(selector) = selector {
        let devices = devices_for(host, kind)?;
        if let Ok(index) = selector.parse::<usize>() {
            let device = devices
                .get(index)
                .cloned()
                .with_context(|| format!("无 {} 设备索引 {index}", kind.label()))?;
            return Ok(SelectedDevice {
                index: Some(index),
                device,
            });
        }
        let needle = selector.to_lowercase();
        return devices
            .into_iter()
            .enumerate()
            .find(|(index, d)| device_matches_selector(d, kind, *index, selector, &needle))
            .map(|(index, device)| SelectedDevice {
                index: Some(index),
                device,
            })
            .with_context(|| format!("无名称含 {selector:?} 的 {} 设备", kind.label()));
    }
    select_default_device(host, kind)
}

pub(super) fn select_reference_source(
    host: &cpal::Host,
    selector: &str,
) -> Result<ReferenceSource> {
    match selector {
        "none" | "" => Ok(ReferenceSource::None),
        "system" | "default" => select_system_reference_source(host),
        sel => {
            let (device, kind) = select_render_device(host, sel)?;
            Ok(ReferenceSource::Cpal { device, kind })
        }
    }
}

#[cfg(target_os = "macos")]
fn select_system_reference_source(_host: &cpal::Host) -> Result<ReferenceSource> {
    Ok(ReferenceSource::ProcessTap)
}

#[cfg(not(target_os = "macos"))]
fn select_system_reference_source(host: &cpal::Host) -> Result<ReferenceSource> {
    Ok(ReferenceSource::Cpal {
        device: select_default_device(host, DeviceKind::Output)
            .context("无默认输出设备可作系统 loopback")?,
        kind: DeviceKind::Output,
    })
}

fn select_render_device(host: &cpal::Host, selector: &str) -> Result<(SelectedDevice, DeviceKind)> {
    if let Some((prefix, sel)) = selector.split_once(':') {
        let kind = match prefix.to_lowercase().as_str() {
            "input" | "in" => DeviceKind::Input,
            "output" | "out" => DeviceKind::Output,
            _ => bail!("参考设备前缀须为 input: 或 output:"),
        };
        return Ok((select_device(host, kind, Some(sel))?, kind));
    }
    if let Ok(d) = select_device(host, DeviceKind::Output, Some(selector)) {
        return Ok((d, DeviceKind::Output));
    }
    select_device(host, DeviceKind::Input, Some(selector)).map(|d| (d, DeviceKind::Input))
}

fn devices_for(host: &cpal::Host, kind: DeviceKind) -> Result<Vec<Device>> {
    match kind {
        DeviceKind::Input => Ok(host.input_devices()?.collect()),
        DeviceKind::Output => Ok(host.output_devices()?.collect()),
    }
}

pub(super) fn pick_config(
    device: &Device,
    kind: DeviceKind,
    sample_rate: u32,
) -> Result<StreamConfigChoice> {
    let ranges: Vec<SupportedStreamConfigRange> = match kind {
        DeviceKind::Input => device.supported_input_configs()?.collect(),
        DeviceKind::Output => device.supported_output_configs()?.collect(),
    };
    let ranges = ranges
        .into_iter()
        .filter(|r| !r.sample_format().is_dsd())
        .collect::<Vec<_>>();

    if let Some(exact) = ranges
        .iter()
        .filter(|r| r.min_sample_rate() <= sample_rate && sample_rate <= r.max_sample_rate())
        .max_by(|a, b| a.cmp_default_heuristics(b))
    {
        return Ok(StreamConfigChoice::new(
            (*exact).with_sample_rate(sample_rate),
            sample_rate,
        ));
    }

    if let Ok(default) = default_config(device, kind) {
        if !default.sample_format().is_dsd() {
            return Ok(StreamConfigChoice::new(default, sample_rate));
        }
    }

    ranges
        .into_iter()
        .map(|range| {
            let chosen_rate = sample_rate
                .max(range.min_sample_rate())
                .min(range.max_sample_rate());
            let distance = chosen_rate.abs_diff(sample_rate);
            (distance, range.with_sample_rate(chosen_rate))
        })
        .min_by(|(a_distance, a), (b_distance, b)| {
            a_distance
                .cmp(b_distance)
                .then_with(|| b.channels().cmp(&a.channels()))
        })
        .map(|(_, config)| StreamConfigChoice::new(config, sample_rate))
        .with_context(|| {
            format!(
                "{} 没有可用的非 DSD {} 配置",
                device_label(device),
                kind.label()
            )
        })
}

fn default_config(device: &Device, kind: DeviceKind) -> Result<SupportedStreamConfig> {
    match kind {
        DeviceKind::Input => device.default_input_config(),
        DeviceKind::Output => device.default_output_config(),
    }
    .with_context(|| format!("{} 没有默认 {} 配置", device_label(device), kind.label()))
}

// ── 设备列表 ──────────────────────────────────────────────────────────────────

pub fn print_devices() -> Result<()> {
    let host = cpal::default_host();
    for kind in [DeviceKind::Input, DeviceKind::Output] {
        println!("{} 设备:", kind.label());
        for (i, d) in devices_for(&host, kind)?.iter().enumerate() {
            let cfg = match kind {
                DeviceKind::Input => d.default_input_config(),
                DeviceKind::Output => d.default_output_config(),
            };
            let summary = cfg
                .map(|c| config_summary(&c))
                .unwrap_or_else(|e| format!("无默认配置: {e}"));
            println!(
                "  {} ({summary})",
                format_indexed_label(Some(i), device_label(d))
            );
        }
    }
    println!(
        "\n用法:run --config 配置文件;也可用 --mic / --reference / --output 覆盖配置里的设备选择。"
    );
    println!(
        "reference 还支持 'system'(Win=默认输出 loopback,mac=Process Tap)/ 'none' / 'output:<名>' / 'input:<名>'。"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct DeviceListOptions {
    pub include_config_details: bool,
}

impl Default for DeviceListOptions {
    fn default() -> Self {
        Self {
            include_config_details: true,
        }
    }
}

pub fn devices_json() -> Result<Value> {
    devices_json_with_options(DeviceListOptions::default())
}

pub fn devices_json_with_options(options: DeviceListOptions) -> Result<Value> {
    let host = cpal::default_host();
    let input_devices = devices_for(&host, DeviceKind::Input)?;
    let output_devices = devices_for(&host, DeviceKind::Output)?;
    let default_input_index = host
        .default_input_device()
        .as_ref()
        .and_then(|device| find_device_index(&input_devices, device));
    let default_output_index = host
        .default_output_device()
        .as_ref()
        .and_then(|device| find_device_index(&output_devices, device));

    let inputs = input_devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            device_json_entry(
                device,
                DeviceKind::Input,
                index,
                default_input_index,
                options,
            )
        })
        .collect::<Vec<_>>();
    let outputs = output_devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            device_json_entry(
                device,
                DeviceKind::Output,
                index,
                default_output_index,
                options,
            )
        })
        .collect::<Vec<_>>();

    let reference_sources =
        reference_sources_json(&input_devices, &output_devices, default_output_index);

    Ok(json!({
        "ok": true,
        "inputs": inputs,
        "outputs": outputs,
        "reference_sources": reference_sources,
    }))
}

#[derive(Clone, Copy, Debug)]
pub struct AudioDoctorOptions {
    pub include_config_details: bool,
    pub request_system_audio: bool,
}

impl Default for AudioDoctorOptions {
    fn default() -> Self {
        Self {
            include_config_details: true,
            request_system_audio: false,
        }
    }
}

pub fn audio_doctor_json_with_options(options: AudioDoctorOptions) -> Result<Value> {
    let devices = devices_json_with_options(DeviceListOptions {
        include_config_details: options.include_config_details,
    })?;
    let inputs = devices["inputs"].as_array().cloned().unwrap_or_default();
    let outputs = devices["outputs"].as_array().cloned().unwrap_or_default();
    let candidate_inputs = inputs
        .iter()
        .filter(|entry| is_virtual_audio_input_name(entry["name"].as_str().unwrap_or_default()))
        .map(audio_candidate_json)
        .collect::<Vec<_>>();
    let candidate_outputs = outputs
        .iter()
        .filter(|entry| is_virtual_audio_output_name(entry["name"].as_str().unwrap_or_default()))
        .map(audio_candidate_json)
        .collect::<Vec<_>>();
    let virtual_output_detected = !candidate_outputs.is_empty();
    let install_status = match (candidate_inputs.is_empty(), candidate_outputs.is_empty()) {
        (false, false) => "installed",
        (true, true) => "missing",
        _ => "unknown",
    };
    // D4:VB-CABLE 装完必须重启 Windows 才出现音频端点。驱动残迹在而端点不在
    // = 「已装未生效」,以 needs_reboot 提示前端向导进入重启节点。
    let driver_present = vb_cable_driver_present();
    let needs_reboot = cfg!(windows) && driver_present && install_status != "installed";
    let system_audio_probe = options
        .request_system_audio
        .then(request_system_audio_permission);
    let system_audio_permission = system_audio_probe
        .as_ref()
        .map(|probe| probe.state)
        .unwrap_or_else(system_audio_permission_state);

    Ok(json!({
        "ok": true,
        "platform": std::env::consts::OS,
        "virtual_output_detected": virtual_output_detected,
        "candidate_outputs": candidate_outputs,
        "candidate_inputs": candidate_inputs,
        "recommended_driver": recommended_audio_driver(),
        "install_status": install_status,
        "needs_reboot": needs_reboot,
        "virtual_driver_present": driver_present,
        "permission_state": audio_permission_state(&inputs),
        "system_audio_permission": system_audio_permission,
        "system_audio_permission_probe": system_audio_probe.as_ref().map(SystemAudioPermissionProbe::to_json),
        "reference_sources": devices["reference_sources"].clone(),
        "hint": audio_doctor_hint(install_status),
    }))
}

struct SystemAudioPermissionProbe {
    state: &'static str,
    ok: bool,
    detail: String,
}

impl SystemAudioPermissionProbe {
    fn to_json(&self) -> Value {
        json!({
            "requested": true,
            "ok": self.ok,
            "state": self.state,
            "detail": self.detail,
        })
    }
}

fn device_json_entry(
    device: &Device,
    kind: DeviceKind,
    index: usize,
    default_index: Option<usize>,
    options: DeviceListOptions,
) -> Value {
    let cfg = options.include_config_details.then(|| match kind {
        DeviceKind::Input => device.default_input_config(),
        DeviceKind::Output => device.default_output_config(),
    });
    let (default_sample_rate, channels, sample_format, config_error) = match cfg {
        Some(Ok(cfg)) => (
            Some(cfg.sample_rate()),
            Some(cfg.channels()),
            Some(cfg.sample_format().to_string()),
            None,
        ),
        Some(Err(err)) => (None, None, None, Some(err.to_string())),
        None => (None, None, None, None),
    };
    let mut entry = json!({
        "id": index.to_string(),
        "stable_id": stable_device_id(device, kind, index),
        "index": index,
        "name": device_label(device),
        "kind": kind.label(),
        "is_default": default_index == Some(index),
        "selector": index.to_string(),
        "default_sample_rate": default_sample_rate,
        "channels": channels,
        "sample_format": sample_format,
        "config_error": config_error,
    });
    if options.include_config_details {
        entry["supported_sample_rates"] = supported_sample_rates_json(device, kind);
    }
    entry
}

fn supported_sample_rates_json(device: &Device, kind: DeviceKind) -> Value {
    match kind {
        DeviceKind::Input => match device.supported_input_configs() {
            Ok(ranges) => supported_ranges_json(ranges),
            Err(err) => json!({ "error": err.to_string() }),
        },
        DeviceKind::Output => match device.supported_output_configs() {
            Ok(ranges) => supported_ranges_json(ranges),
            Err(err) => json!({ "error": err.to_string() }),
        },
    }
}

fn supported_ranges_json(ranges: impl Iterator<Item = SupportedStreamConfigRange>) -> Value {
    Value::Array(
        ranges
            .filter(|range| !range.sample_format().is_dsd())
            .map(|range| {
                json!({
                    "min": range.min_sample_rate(),
                    "max": range.max_sample_rate(),
                    "channels": range.channels(),
                    "sample_format": range.sample_format().to_string(),
                })
            })
            .collect(),
    )
}

fn reference_sources_json(
    input_devices: &[Device],
    output_devices: &[Device],
    default_output_index: Option<usize>,
) -> Vec<Value> {
    let mut reference_sources = Vec::new();
    if !cfg!(target_os = "linux") {
        reference_sources.push(system_reference_source(default_output_index.is_some()));
    }
    reference_sources.push(no_reference_source());

    if cfg!(target_os = "linux") {
        reference_sources.extend(
            input_devices
                .iter()
                .enumerate()
                .filter_map(|(index, device)| {
                    let label = device_label(device);
                    is_linux_monitor_source_name(&label).then(|| {
                        input_reference_source_json(
                            index,
                            label,
                            stable_device_id(device, DeviceKind::Input, index),
                            ReferenceIdStyle::Label,
                        )
                    })
                }),
        );
        return reference_sources;
    }

    reference_sources.extend(input_devices.iter().enumerate().map(|(index, device)| {
        input_reference_source_json(
            index,
            device_label(device),
            stable_device_id(device, DeviceKind::Input, index),
            ReferenceIdStyle::Index,
        )
    }));
    if !cfg!(target_os = "macos") {
        reference_sources.extend(output_devices.iter().enumerate().map(|(index, device)| {
            output_reference_source_json(
                index,
                device_label(device),
                stable_device_id(device, DeviceKind::Output, index),
            )
        }));
    }
    reference_sources
}

fn no_reference_source() -> Value {
    json!({
        "id": "none",
        "stable_id": "none",
        "label": "No reference",
        "kind": "none",
        "available": true,
        "hint": "No far-end reference; AEC will behave like single-ended processing."
    })
}

enum ReferenceIdStyle {
    Index,
    Label,
}

fn input_reference_source_json(
    index: usize,
    label: String,
    stable_id: String,
    id_style: ReferenceIdStyle,
) -> Value {
    let id = match id_style {
        ReferenceIdStyle::Index => format!("input:{index}"),
        ReferenceIdStyle::Label => format!("input:{label}"),
    };
    json!({
        "id": id,
        "stable_id": format!("input:{stable_id}"),
        "label": label,
        "kind": "input",
        "device_index": index,
        "selector": format!("input:{stable_id}"),
        "available": true,
    })
}

fn output_reference_source_json(index: usize, label: String, stable_id: String) -> Value {
    json!({
        "id": format!("output:{index}"),
        "stable_id": format!("output:{stable_id}"),
        "label": label,
        "kind": "output",
        "device_index": index,
        "selector": format!("output:{stable_id}"),
        "available": true,
    })
}

fn system_reference_source(has_default_output: bool) -> Value {
    let available = if cfg!(windows) {
        has_default_output
    } else if cfg!(target_os = "macos") {
        macos_process_tap_helper_available()
    } else {
        false
    };
    let hint = if cfg!(windows) {
        if has_default_output {
            "Windows default render endpoint loopback is available."
        } else {
            "No default output device is available for system loopback."
        }
    } else if cfg!(target_os = "macos") {
        if available {
            "macOS Process Tap helper is available; first use may request System Audio Recording permission."
        } else {
            "macOS Process Tap helper is not bundled/built; use BlackHole/VB-CABLE MAC fallback or build the helper."
        }
    } else {
        "System loopback availability depends on the platform backend; use an explicit routed reference source if unavailable."
    };
    json!({
        "id": "system",
        "stable_id": "system",
        "label": if cfg!(target_os = "macos") { "System Audio (Process Tap)" } else { "System audio" },
        "kind": "system",
        "available": available,
        "hint": hint,
    })
}

fn macos_process_tap_helper_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_process_tap::helper_available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn audio_candidate_json(entry: &Value) -> Value {
    json!({
        "name": entry["name"].as_str().unwrap_or_default(),
        "selector": entry["selector"].as_str().unwrap_or_default(),
        "stable_id": entry["stable_id"].as_str().unwrap_or_default(),
        "index": entry["index"].as_u64(),
        "kind": entry["kind"].as_str().unwrap_or_default(),
    })
}

fn recommended_audio_driver() -> &'static str {
    if cfg!(windows) {
        "vb-cable"
    } else if cfg!(target_os = "macos") {
        "blackhole-2ch"
    } else if cfg!(target_os = "linux") {
        "pipewire-null-sink"
    } else {
        "unknown"
    }
}

fn audio_permission_state(inputs: &[Value]) -> &'static str {
    if cfg!(target_os = "macos") {
        if inputs.is_empty() {
            "undetermined"
        } else {
            "granted"
        }
    } else {
        "unknown"
    }
}

pub(super) fn system_audio_permission_state() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        if macos_process_tap_helper_available() {
            // 真实 TCC 预检(无弹窗)。查询失败才回退 undetermined,
            // 让 UI 保留「请求权限」入口而不是误报 granted。
            return macos_process_tap::preflight_permission().unwrap_or("undetermined");
        }
        "unknown"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unknown"
    }
}

// VB-CABLE 驱动残迹检测(仅 Windows):装完未重启时端点枚举不到,
// 但驱动 sys 文件 / 安装目录已落盘 —— 用它区分「没装」和「装了没生效」。
fn vb_cable_driver_present() -> bool {
    #[cfg(windows)]
    {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let sys_files = [
            format!(r"{windir}\System32\drivers\vbaudio_cable64_win7.sys"),
            format!(r"{windir}\System32\drivers\vbaudio_cable32_win7.sys"),
        ];
        if sys_files.iter().any(|p| std::path::Path::new(p).is_file()) {
            return true;
        }
        // 卸载程序所在的安装目录兜底(不同版本 sys 文件名可能变)。
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .any(|pf| std::path::Path::new(&pf).join(r"VB\CABLE").is_dir())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn request_system_audio_permission() -> SystemAudioPermissionProbe {
    #[cfg(target_os = "macos")]
    {
        match macos_process_tap::probe_permission() {
            Ok(detail) => SystemAudioPermissionProbe {
                state: "granted",
                ok: true,
                detail,
            },
            Err(err) => {
                let detail = format!("{err:#}");
                let state = if detail.contains("未找到 macOS Process Tap helper")
                    || detail.contains("Process Tap availability")
                {
                    "unknown"
                } else {
                    "denied"
                };
                SystemAudioPermissionProbe {
                    state,
                    ok: false,
                    detail,
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        SystemAudioPermissionProbe {
            state: "unknown",
            ok: false,
            detail: "System Audio Recording permission is macOS-only.".to_string(),
        }
    }
}

fn audio_doctor_hint(install_status: &str) -> &'static str {
    if cfg!(target_os = "linux") {
        return "Create the PipeWire/PulseAudio null sink with: pactl load-module module-null-sink sink_name=echoless_out sink_properties=device.description=Echoless-Output. In your call app, choose \"Monitor of Echoless-Output\" as the microphone.";
    }
    match install_status {
        "installed" => "Virtual audio input/output candidates were detected.",
        "missing" => {
            if cfg!(target_os = "macos") {
                "No virtual audio candidates detected; install BlackHole 2ch or VB-CABLE MAC, then refresh devices."
            } else if cfg!(windows) {
                "No virtual audio candidates detected; install VB-Audio VB-CABLE, then refresh devices."
            } else {
                "No virtual audio candidates detected; configure a platform virtual audio route, then refresh devices."
            }
        }
        _ => "Only one side of a virtual audio route was detected; verify the driver and refresh devices.",
    }
}

fn is_virtual_audio_input_name(name: &str) -> bool {
    if cfg!(target_os = "linux") {
        return is_linux_monitor_source_name(name);
    }
    is_virtual_audio_name(name)
}

fn is_virtual_audio_output_name(name: &str) -> bool {
    if cfg!(target_os = "linux") {
        return is_linux_virtual_output_name(name);
    }
    is_virtual_audio_name(name)
}

fn is_linux_monitor_source_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("monitor")
}

fn is_linux_virtual_output_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("echoless") || name.contains("null")
}

pub(super) fn is_virtual_audio_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("vb-audio")
        || name.contains("vb-cable")
        || name.contains("vbcable")
        || name.contains("cable input")
        || name.contains("cable output")
        || name.contains("blackhole")
        || name.contains("virtual desktop")
}

fn stable_device_id(device: &Device, kind: DeviceKind, index: usize) -> String {
    if let Ok(id) = device.id() {
        let id = id.to_string();
        if !id.trim().is_empty() {
            return id;
        }
    }
    if let Ok(desc) = device.description() {
        if let Some(address) = desc.address().filter(|value| !value.trim().is_empty()) {
            return format!("{}:{}", kind.label(), address.trim());
        }
    }
    format!(
        "{}:name:{}",
        kind.label(),
        normalize_device_id_part(&device_label(device), index)
    )
}

fn normalize_device_id_part(label: &str, index: usize) -> String {
    let mut out = String::new();
    for ch in label.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        format!("device-{index}")
    } else {
        out.to_string()
    }
}

fn config_summary(c: &SupportedStreamConfig) -> String {
    format!(
        "{} Hz, {} ch, {}",
        c.sample_rate(),
        c.channels(),
        c.sample_format()
    )
}

pub(super) fn config_choice_summary(c: &StreamConfigChoice) -> String {
    let base = config_summary(&c.supported);
    if c.requires_resampling() {
        format!("{base}, resample to {} Hz pipeline", c.pipeline_sample_rate)
    } else {
        base
    }
}

pub(super) fn selected_device_label(selected: &SelectedDevice) -> String {
    format_indexed_label(selected.index, device_label(&selected.device))
}

fn format_indexed_label(index: Option<usize>, label: String) -> String {
    match index {
        Some(index) => format!("[{index}] {label}"),
        None => label,
    }
}

fn device_label(device: &Device) -> String {
    device
        .description()
        .map(|d| format_device_description(&d))
        .unwrap_or_else(|_| "<未知>".to_owned())
}

pub(super) fn format_device_description(desc: &DeviceDescription) -> String {
    let name = desc.name().trim();
    let detail = desc
        .driver()
        .filter(|v| distinct_label(name, v))
        .or_else(|| {
            desc.extended()
                .iter()
                .map(String::as_str)
                .find(|v| distinct_label(name, v))
        })
        .or_else(|| desc.manufacturer().filter(|v| distinct_label(name, v)));

    match detail {
        Some(detail) => format!("{name} / {}", detail.trim()),
        None => {
            let display = desc.to_string();
            let display = display.trim();
            if display.is_empty() {
                name.to_owned()
            } else {
                display.to_owned()
            }
        }
    }
}

fn distinct_label(primary: &str, candidate: &str) -> bool {
    let primary = primary.trim();
    let candidate = candidate.trim();
    !candidate.is_empty() && !candidate.eq_ignore_ascii_case(primary)
}

fn device_search_text(device: &Device) -> String {
    let mut parts = Vec::new();
    if let Ok(desc) = device.description() {
        parts.push(desc.name().to_owned());
        parts.extend(desc.manufacturer().map(str::to_owned));
        parts.extend(desc.driver().map(str::to_owned));
        parts.extend(desc.address().map(str::to_owned));
        parts.extend(desc.extended().iter().cloned());
        parts.push(desc.to_string());
    }
    if let Ok(id) = device.id() {
        parts.push(id.to_string());
    }
    parts.join(" ")
}

fn device_matches_selector(
    device: &Device,
    kind: DeviceKind,
    index: usize,
    selector: &str,
    lower_selector: &str,
) -> bool {
    stable_device_id(device, kind, index).eq_ignore_ascii_case(selector)
        || device_search_text(device)
            .to_lowercase()
            .contains(lower_selector)
}

fn find_device_index(devices: &[Device], selected: &Device) -> Option<usize> {
    selected.id().ok().and_then(|id| {
        devices
            .iter()
            .position(|device| device.id().ok().as_ref() == Some(&id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_monitor_reference_uses_input_label_id() {
        let source = input_reference_source_json(
            2,
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor".to_string(),
            "name:alsa-output-monitor".to_string(),
            ReferenceIdStyle::Label,
        );

        assert_eq!(
            source["id"],
            "input:alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
        );
        assert_eq!(source["kind"], "input");
        assert_eq!(source["selector"], "input:name:alsa-output-monitor");
    }

    #[test]
    fn linux_audio_name_helpers_match_monitor_and_null_sink() {
        assert!(is_linux_monitor_source_name(
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
        ));
        assert!(is_linux_monitor_source_name("Monitor of Echoless-Output"));
        assert!(is_linux_virtual_output_name("Echoless-Output"));
        assert!(is_linux_virtual_output_name("Null Output"));
        assert!(!is_linux_monitor_source_name("Built-in Microphone"));
        assert!(!is_linux_virtual_output_name(
            "Built-in Audio Analog Stereo"
        ));
    }
}
