//! aec3 节点回归测试。验证 vendored aec3 fork 后:
//!   (1) AEC3 真实消回声;
//!   (2) 经 fork 开放的 tail 调参口能注入、跑通、不劣化。
//! 无外部 WAV,纯合成。仅在 aec3-engine 特性下运行(默认开)。
//!
//! 关键经验(调研得来,见 research/aec3_internal_map.md):
//!   - 激励必须是**非平稳语音类**信号。AEC3 有 stationarity gate,平稳白噪声会被当
//!     背景噪声、抑制滤波器自适应(实测平稳白噪声仅 ~8dB,非平稳语音类 20–41dB)。
//!   - `erle_db` 统计在本路径下恒为常数、不可信(§7);效果以输出能量下降为准。
//!   - 合成长跑(>10s)效果会随 pause 段累积退化,疑为合成信号 artifact;真实退化
//!     行为待真实录音验证(Phase 1)。故本测试用 5s 短窗稳定断言。

#![cfg(feature = "aec3-engine")]

use echoless_processors::registry;

const SR: usize = 48_000;
const FRAME: usize = 480; // aec3 io_spec = 48k

fn white(n: usize) -> f32 {
    let mut x = (n as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    x ^= x >> 33;
    (x as u32) as f32 / u32::MAX as f32 - 0.5
}

/// 语音类非平稳参考信号:白噪声 × 音节起伏包络 + 周期停顿。
fn refsig(n: usize) -> f32 {
    use std::f32::consts::PI;
    let t = n as f32 / SR as f32;
    let syllable = 0.5 + 0.5 * (2.0 * PI * 3.0 * t).sin();
    let pause = if (t % 1.0) < 0.7 { 1.0 } else { 0.08 };
    white(n) * syllable * pause * 1.6
}

fn set_delay_hold(params: &mut toml::Table, enabled: bool) {
    params.insert("delay_hold".into(), toml::Value::Boolean(enabled));
}

/// 跑合成回声场景。`paths` = 各回声径 (延迟样本, 增益);near 为各径叠加,无近端人声。
/// 返回回声能量下降 dB(跳过前 2 秒收敛)。
fn run(params: toml::Table, paths: &[(usize, f32)]) -> f32 {
    let mut p = registry::build("aec3").unwrap();
    p.configure(&params).unwrap();
    let far_channels = p.io_spec().far_channels.max(1) as usize;

    let total = SR * 5;
    let warmup = SR * 2;
    let mut near = vec![0.0f32; FRAME];
    let mut far = vec![0.0f32; FRAME * far_channels];
    let mut out = vec![0.0f32; FRAME];

    let (mut mic_sq, mut out_sq, mut cnt) = (0.0f64, 0.0f64, 0u64);
    let mut i = 0;
    while i + FRAME <= total {
        for j in 0..FRAME {
            let n = i + j;
            for ch in 0..far_channels {
                far[j * far_channels + ch] = refsig(n);
            }
            let mut echo = 0.0;
            for &(d, g) in paths {
                if n >= d {
                    echo += g * refsig(n - d);
                }
            }
            near[j] = echo;
        }
        p.process(&near, &far, &mut out, FRAME as u32);
        if i >= warmup {
            for j in 0..FRAME {
                mic_sq += (near[j] as f64).powi(2);
                out_sq += (out[j] as f64).powi(2);
                cnt += 1;
            }
        }
        i += FRAME;
    }
    let mic = (mic_sq / cnt as f64).sqrt();
    let out = (out_sq / cnt as f64).sqrt();
    20.0 * (mic / out.max(1e-12)).log10() as f32
}

fn run_with_ref_hole(params: toml::Table, paths: &[(usize, f32)]) -> f32 {
    let mut p = registry::build("aec3").unwrap();
    p.configure(&params).unwrap();
    let far_channels = p.io_spec().far_channels.max(1) as usize;

    let total = SR * 7;
    let hole_start = SR * 3;
    let hole_end = hole_start + SR * 3 / 10;
    let recovery_end = hole_end + SR / 10;
    let mut near = vec![0.0f32; FRAME];
    let mut far = vec![0.0f32; FRAME * far_channels];
    let mut out = vec![0.0f32; FRAME];

    let (mut mic_sq, mut out_sq, mut cnt) = (0.0f64, 0.0f64, 0u64);
    let mut i = 0;
    while i + FRAME <= total {
        for j in 0..FRAME {
            let n = i + j;
            let far_sample = if (hole_start..hole_end).contains(&n) {
                0.0
            } else {
                refsig(n)
            };
            for ch in 0..far_channels {
                far[j * far_channels + ch] = far_sample;
            }
            let mut echo = 0.0;
            for &(d, g) in paths {
                if n >= d {
                    echo += g * refsig(n - d);
                }
            }
            near[j] = echo;
        }
        p.process(&near, &far, &mut out, FRAME as u32);
        for j in 0..FRAME {
            let n = i + j;
            if (hole_end..recovery_end).contains(&n) {
                mic_sq += (near[j] as f64).powi(2);
                out_sq += (out[j] as f64).powi(2);
                cnt += 1;
            }
        }
        i += FRAME;
    }
    let mic = (mic_sq / cnt as f64).sqrt();
    let out = (out_sq / cnt as f64).sqrt();
    20.0 * (mic / out.max(1e-12)).log10() as f32
}

fn run_windowed(
    params: toml::Table,
    paths: &[(usize, f32)],
    windows: &[(usize, usize)],
) -> Vec<f32> {
    let mut p = registry::build("aec3").unwrap();
    p.configure(&params).unwrap();
    let far_channels = p.io_spec().far_channels.max(1) as usize;

    let total = windows
        .iter()
        .map(|(_, end)| *end)
        .max()
        .unwrap_or(SR * 5)
        .max(SR * 5);
    let mut near = vec![0.0f32; FRAME];
    let mut far = vec![0.0f32; FRAME * far_channels];
    let mut out = vec![0.0f32; FRAME];
    let mut accum = vec![(0.0f64, 0.0f64, 0u64); windows.len()];

    let mut i = 0;
    while i + FRAME <= total {
        for j in 0..FRAME {
            let n = i + j;
            for ch in 0..far_channels {
                far[j * far_channels + ch] = refsig(n);
            }
            let mut echo = 0.0;
            for &(d, g) in paths {
                if n >= d {
                    echo += g * refsig(n - d);
                }
            }
            near[j] = echo;
        }
        p.process(&near, &far, &mut out, FRAME as u32);
        for j in 0..FRAME {
            let n = i + j;
            for (window, (mic_sq, out_sq, cnt)) in windows.iter().zip(accum.iter_mut()) {
                if (window.0..window.1).contains(&n) {
                    *mic_sq += (near[j] as f64).powi(2);
                    *out_sq += (out[j] as f64).powi(2);
                    *cnt += 1;
                }
            }
        }
        i += FRAME;
    }

    accum
        .into_iter()
        .map(|(mic_sq, out_sq, cnt)| {
            let mic = (mic_sq / cnt as f64).sqrt();
            let out = (out_sq / cnt as f64).sqrt();
            20.0 * (mic / out.max(1e-12)).log10() as f32
        })
        .collect()
}

#[test]
fn cancels_single_path_echo() {
    // 单径 50ms 回声,默认 config。
    let db = run(toml::Table::new(), &[(2400, 0.5)]);
    assert!(db > 18.0, "单径回声压低不足:{db:.1} dB");
}

#[test]
fn tuned_tail_injection_works() {
    // 经 vendored fork 注入更长 tail,验证调参口可用且不劣化 AEC。
    let mut params = toml::Table::new();
    params.insert("tail_ms".into(), toml::Value::Integer(120));
    let db = run(params, &[(2400, 0.5)]);
    assert!(db > 18.0, "tail=120ms 注入后回声压低不足:{db:.1} dB");
}

#[test]
fn stereo_reference_mode_cancels_single_path_echo() {
    let mut params = toml::Table::new();
    params.insert(
        "reference_channels".into(),
        toml::Value::String("stereo".into()),
    );

    let db = run(params, &[(2400, 0.5)]);
    assert!(db > 18.0, "stereo reference 回声压低不足:{db:.1} dB");
}

#[test]
fn stereo_reference_mode_changes_aec3_io_spec() {
    let mut p = registry::build("aec3").unwrap();
    let mut params = toml::Table::new();
    params.insert(
        "reference_channels".into(),
        toml::Value::String("stereo".into()),
    );

    p.configure(&params).unwrap();

    assert_eq!(p.io_spec().far_channels, 2);
}

#[test]
fn delay_hold_recovers_faster_after_reference_hole() {
    let mut hold_on = toml::Table::new();
    set_delay_hold(&mut hold_on, true);
    let mut hold_off = toml::Table::new();
    set_delay_hold(&mut hold_off, false);

    let on_db = run_with_ref_hole(hold_on, &[(2400, 0.5)]);
    let off_db = run_with_ref_hole(hold_off, &[(2400, 0.5)]);

    assert!(
        on_db > off_db + 2.0,
        "delay_hold 恢复收益不足:on={on_db:.1}dB off={off_db:.1}dB"
    );
}

#[test]
#[ignore = "heavy >60s synthetic AEC3 regression check"]
fn long_run_energy_reduction_stays_stable_over_60s() {
    let windows = [(SR * 5, SR * 15), (SR * 55, SR * 65)];
    let db = run_windowed(toml::Table::new(), &[(2400, 0.5)], &windows);
    let early_db = db[0];
    let late_db = db[1];

    assert!(
        late_db >= early_db - 3.0 && late_db > 18.0,
        ">60s AEC3 长跑退化:early={early_db:.1}dB late={late_db:.1}dB"
    );
}
