//! 直接调 aec3 高层 API 的诊断测试(绕过我们的 wrapper),用于隔离
//! "aec3 本身 vs 我们 wrapper 调用方式" 的问题。多场景对比。

#![cfg(feature = "aec3-engine")]

use aec3_apm::config::{EchoCanceller, Pipeline};
use aec3_apm::{AudioProcessing, Config, StreamConfig};

const SR: usize = 48_000;
const FRAME: usize = 480;

fn white(n: usize) -> f32 {
    let mut x = (n as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    x ^= x >> 33;
    (x as u32) as f32 / u32::MAX as f32 - 0.5
}

// 语音类非平稳信号:白噪声 × 缓慢幅度包络(音节起伏)+ 周期停顿。
// AEC3 靠非平稳激发滤波器自适应(平稳噪声会被 stationarity gate 抑制更新)。
fn refsig(n: usize) -> f32 {
    use std::f32::consts::PI;
    let t = n as f32 / SR as f32;
    let syllable = 0.5 + 0.5 * (2.0 * PI * 3.0 * t).sin(); // 3Hz 音节起伏
    let pause = if (t % 1.0) < 0.7 { 1.0 } else { 0.08 }; // 每秒后 0.3s 近似停顿
    white(n) * syllable * pause * 1.6
}

/// delay 样本数;far_stereo=true 时 render 2ch(L=R),否则 mono。
fn run(delay: usize, far_stereo: bool, set_delay_ms: Option<i32>) -> (f32, f32, f32) {
    let render_ch: u16 = if far_stereo { 2 } else { 1 };
    let config = Config {
        echo_canceller: Some(EchoCanceller::default()),
        pipeline: Pipeline {
            multi_channel_render: far_stereo,
            multi_channel_capture: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut apm = AudioProcessing::builder()
        .config(config)
        .capture_config(StreamConfig::new(48000, 1))
        .render_config(StreamConfig::new(48000, render_ch))
        .echo_detector(true)
        .build();
    if let Some(ms) = set_delay_ms {
        let _ = apm.set_stream_delay_ms(ms);
    }

    let total = SR * 5;
    let warmup = SR * 2;
    let mut near = vec![0.0f32; FRAME];
    let mut far_l = vec![0.0f32; FRAME];
    let mut far_r = vec![0.0f32; FRAME];
    let mut out = vec![0.0f32; FRAME];
    let mut fo_l = vec![0.0f32; FRAME];
    let mut fo_r = vec![0.0f32; FRAME];

    let (mut mic_sq, mut out_sq, mut cnt) = (0.0f64, 0.0f64, 0u64);
    let mut i = 0;
    while i + FRAME <= total {
        for j in 0..FRAME {
            let n = i + j;
            let r = refsig(n);
            far_l[j] = r;
            far_r[j] = r;
            near[j] = if n >= delay {
                0.5 * refsig(n - delay)
            } else {
                0.0
            };
        }
        if far_stereo {
            let _ = apm.process_render_f32(&[&far_l, &far_r], &mut [&mut fo_l, &mut fo_r]);
        } else {
            let _ = apm.process_render_f32(&[&far_l], &mut [&mut fo_l]);
        }
        let _ = apm.process_capture_f32(&[&near], &mut [&mut out]);
        if i >= warmup {
            for j in 0..FRAME {
                mic_sq += (near[j] as f64).powi(2);
                out_sq += (out[j] as f64).powi(2);
                cnt += 1;
            }
        }
        i += FRAME;
    }
    let mic = (mic_sq / cnt as f64).sqrt() as f32;
    let o = (out_sq / cnt as f64).sqrt() as f32;
    let db = 20.0 * (mic / o.max(1e-9)).log10();
    let s = apm.statistics();
    eprintln!(
        "delay={delay} stereo={far_stereo} set_delay={set_delay_ms:?} -> reduction={db:.1}dB \
         erle={:?} est_delay={:?} resid={:?}",
        s.echo_return_loss_enhancement, s.delay_ms, s.residual_echo_likelihood
    );
    (mic, o, db)
}

/// 引擎健康断言:直接 aec3 高层 API,非平稳信号 50ms 回声应消 >18dB。
/// (与经 wrapper 的 echo_cancellation 测试互为对照,隔离 wrapper vs 引擎问题。)
#[test]
fn engine_cancels_nonstationary_echo() {
    // mono far,对齐 aec3 节点的 mono 配置。
    let (_, _, db) = run(2400, false, None);
    assert!(db > 18.0, "aec3 引擎对非平稳回声压低不足:{db:.1} dB");
}

/// 诊断矩阵(探查工具,非回归断言)。记录关键经验:
///
/// - 平稳白噪声仅 ~8dB(stationarity gate 抑制自适应);非平稳语音类 20–41dB。
/// - erle_db 统计恒常数、不可信;延迟估计在零延迟 corner 不准。
/// - 50ms(逼近默认 tail 52ms)明显差于 10ms → 印证调大 tail 的价值。
///
/// 跑:`cargo test -p echoless-processors --test aec3_direct -- --ignored --nocapture`
#[test]
#[ignore = "诊断探查工具,非回归断言;手动 --ignored 运行"]
fn diagnostic_matrix() {
    eprintln!("=== 直接 aec3 API 诊断矩阵 ===");
    run(0, false, None); // 零延迟 mono — 最简单
    run(0, true, None); // 零延迟 stereo
    run(480, false, None); // 10ms mono
    run(2400, false, None); // 50ms mono
    run(2400, true, None); // 50ms stereo(= 我们 wrapper 的场景)
    run(2400, true, Some(50)); // 50ms stereo + 提示延迟
}
