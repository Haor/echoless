use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use ringbuf::traits::Consumer;
use rubato::{FftFixedIn, FftFixedOut, Resampler};

pub(super) struct InterleavedInputResampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    configured_input_frames: usize,
    planes: Vec<Vec<f32>>,
    resampled_planes: Vec<Vec<f32>>,
    output: Vec<f32>,
    resampler: Option<FftFixedIn<f32>>,
    linear_fallback: InterleavedLinearFallback,
}

impl InterleavedInputResampler {
    pub(super) fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            in_rate,
            out_rate,
            channels,
            configured_input_frames: 0,
            planes: Vec::new(),
            resampled_planes: Vec::new(),
            output: Vec::new(),
            resampler: None,
            linear_fallback: InterleavedLinearFallback::new(in_rate, out_rate, channels),
        }
    }

    pub(super) fn process(&mut self, input: &[f32]) -> &[f32] {
        self.output.clear();
        if input.is_empty() {
            return &self.output;
        }
        if self.in_rate == self.out_rate {
            self.output.extend_from_slice(input);
            return &self.output;
        }

        let frames = input.len() / self.channels;
        if frames == 0 {
            return &self.output;
        }

        self.ensure_layout(frames);
        self.fill_planes(input, frames);

        if let Some((_, out_frames)) = self.resampler.as_mut().and_then(|resampler| {
            resampler
                .process_into_buffer(&self.planes, &mut self.resampled_planes, None)
                .ok()
        }) {
            self.interleave_from_resampled(out_frames);
        } else {
            self.linear_fallback.process_into(input, &mut self.output);
        }

        &self.output
    }

    fn ensure_layout(&mut self, input_frames: usize) {
        if self.configured_input_frames == input_frames {
            return;
        }
        self.configured_input_frames = input_frames;
        resize_planes(&mut self.planes, self.channels, input_frames);

        self.resampler = FftFixedIn::<f32>::new(
            self.in_rate as usize,
            self.out_rate as usize,
            input_frames,
            1,
            self.channels,
        )
        .ok();
        let output_frames_max = self
            .resampler
            .as_ref()
            .map(Resampler::output_frames_max)
            .unwrap_or_else(|| scaled_frames(input_frames, self.in_rate, self.out_rate));
        resize_planes(&mut self.resampled_planes, self.channels, output_frames_max);
        self.output.resize(output_frames_max * self.channels, 0.0);
        self.output.clear();
    }

    fn fill_planes(&mut self, input: &[f32], frames: usize) {
        for frame in 0..frames {
            let frame_start = frame * self.channels;
            for channel in 0..self.channels {
                self.planes[channel][frame] = input[frame_start + channel];
            }
        }
    }

    fn interleave_from_resampled(&mut self, frames: usize) {
        self.output.resize(frames * self.channels, 0.0);
        for frame in 0..frames {
            for channel in 0..self.channels {
                self.output[frame * self.channels + channel] =
                    self.resampled_planes[channel][frame];
            }
        }
    }
}

struct InterleavedLinearFallback {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    input_frames_seen: u64,
    next_output_source_pos: f64,
    prev_frame: Option<Vec<f32>>,
}

impl InterleavedLinearFallback {
    fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        Self {
            in_rate,
            out_rate,
            channels: channels.max(1),
            input_frames_seen: 0,
            next_output_source_pos: 0.0,
            prev_frame: None,
        }
    }

    fn process_into(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if self.in_rate == self.out_rate || input.is_empty() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / self.channels;
        if frames == 0 {
            return;
        }

        let start_abs = self.input_frames_seen;
        let end_abs = start_abs + frames as u64;
        let step = self.in_rate as f64 / self.out_rate as f64;
        out.reserve(((frames as f64) / step).ceil() as usize * self.channels);

        while self.next_output_source_pos.floor() as u64 + 1 < end_abs {
            let pos = self.next_output_source_pos;
            let i0 = pos.floor() as u64;
            let i1 = i0 + 1;
            let frac = (pos - i0 as f64) as f32;
            for ch in 0..self.channels {
                let a = self.sample_at(input, start_abs, i0, ch).unwrap_or(0.0);
                let b = self.sample_at(input, start_abs, i1, ch).unwrap_or(a);
                out.push(a + (b - a) * frac);
            }
            self.next_output_source_pos += step;
        }

        self.input_frames_seen = end_abs;
        let last_start = (frames - 1) * self.channels;
        self.prev_frame = Some(input[last_start..last_start + self.channels].to_vec());
    }

    fn sample_at(
        &self,
        input: &[f32],
        start_abs: u64,
        index_abs: u64,
        channel: usize,
    ) -> Option<f32> {
        if index_abs + 1 == start_abs {
            return self
                .prev_frame
                .as_ref()
                .and_then(|frame| frame.get(channel).copied());
        }
        if index_abs < start_abs {
            return None;
        }
        let local = (index_abs - start_abs) as usize;
        input.get(local * self.channels + channel).copied()
    }
}

pub(super) struct OutputDeviceResampler {
    in_rate: u32,
    out_rate: u32,
    configured_output_frames: usize,
    input_planes: Vec<Vec<f32>>,
    output_planes: Vec<Vec<f32>>,
    output: Vec<f32>,
    resampler: Option<FftFixedOut<f32>>,
    linear_fallback: OutputLinearFallback,
}

impl OutputDeviceResampler {
    pub(super) fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            in_rate,
            out_rate,
            configured_output_frames: 0,
            input_planes: Vec::new(),
            output_planes: Vec::new(),
            output: Vec::new(),
            resampler: None,
            linear_fallback: OutputLinearFallback::new(in_rate, out_rate),
        }
    }

    pub(super) fn next_chunk<C>(
        &mut self,
        output_frames: usize,
        consumer: &mut C,
        underruns: &AtomicU64,
    ) -> &[f32]
    where
        C: Consumer<Item = f32>,
    {
        self.output.clear();
        if output_frames == 0 {
            return &self.output;
        }
        if self.in_rate == self.out_rate {
            self.output.resize(output_frames, 0.0);
            fill_from_consumer(&mut self.output, consumer, underruns);
            return &self.output;
        }

        self.ensure_layout(output_frames);
        if let Some(resampler) = self.resampler.as_mut() {
            let input_frames = resampler.input_frames_next();
            resize_planes(&mut self.input_planes, 1, input_frames);
            fill_from_consumer(&mut self.input_planes[0], consumer, underruns);
            if let Ok((_, out_frames)) =
                resampler.process_into_buffer(&self.input_planes, &mut self.output_planes, None)
            {
                self.output
                    .extend_from_slice(&self.output_planes[0][..out_frames.min(output_frames)]);
                self.output.resize(output_frames, 0.0);
                return &self.output;
            }
            self.output.resize(output_frames, 0.0);
            return &self.output;
        }

        self.linear_fallback
            .fill_chunk(output_frames, consumer, underruns, &mut self.output);
        &self.output
    }

    fn ensure_layout(&mut self, output_frames: usize) {
        if self.configured_output_frames == output_frames {
            return;
        }
        self.configured_output_frames = output_frames;
        self.resampler = FftFixedOut::<f32>::new(
            self.in_rate as usize,
            self.out_rate as usize,
            output_frames,
            1,
            1,
        )
        .ok();
        let input_frames = self
            .resampler
            .as_ref()
            .map(Resampler::input_frames_next)
            .unwrap_or_else(|| scaled_frames(output_frames, self.out_rate, self.in_rate));
        let output_frames_max = self
            .resampler
            .as_ref()
            .map(Resampler::output_frames_max)
            .unwrap_or(output_frames);
        resize_planes(&mut self.input_planes, 1, input_frames);
        resize_planes(&mut self.output_planes, 1, output_frames_max);
        self.output.resize(output_frames, 0.0);
        self.output.clear();
    }
}

struct OutputLinearFallback {
    step: f64,
    pos: f64,
    buffer: VecDeque<f32>,
}

impl OutputLinearFallback {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        let step = if out_rate == 0 {
            1.0
        } else {
            in_rate as f64 / out_rate as f64
        };
        Self {
            step,
            pos: 0.0,
            buffer: VecDeque::new(),
        }
    }

    fn fill_chunk<C>(
        &mut self,
        output_frames: usize,
        consumer: &mut C,
        underruns: &AtomicU64,
        output: &mut Vec<f32>,
    ) where
        C: Consumer<Item = f32>,
    {
        output.resize(output_frames, 0.0);
        for sample in output.iter_mut() {
            *sample = self.next_sample(consumer, underruns);
        }
    }

    fn next_sample<C>(&mut self, consumer: &mut C, underruns: &AtomicU64) -> f32
    where
        C: Consumer<Item = f32>,
    {
        let needed = (self.pos.floor() as usize).saturating_add(2);
        while self.buffer.len() < needed {
            match consumer.try_pop() {
                Some(sample) => self.buffer.push_back(sample.clamp(-1.0, 1.0)),
                None => {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    return 0.0;
                }
            }
        }

        let i0 = self.pos.floor() as usize;
        let frac = (self.pos - i0 as f64) as f32;
        let a = self.buffer.get(i0).copied().unwrap_or(0.0);
        let b = self.buffer.get(i0 + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac).clamp(-1.0, 1.0);

        self.pos += self.step;
        let consumed = self.pos.floor() as usize;
        for _ in 0..consumed {
            let _ = self.buffer.pop_front();
        }
        self.pos -= consumed as f64;
        sample
    }
}

fn fill_from_consumer<C>(output: &mut [f32], consumer: &mut C, underruns: &AtomicU64)
where
    C: Consumer<Item = f32>,
{
    for sample in output.iter_mut() {
        *sample = match consumer.try_pop() {
            Some(v) => v.clamp(-1.0, 1.0),
            None => {
                underruns.fetch_add(1, Ordering::Relaxed);
                0.0
            }
        };
    }
}

/// 水位反馈速率控制器:PI + 软死区 + 抗饱和钳位。输出侧与参考侧共用。
///
/// 输出 trim = 有效重采样比率相对基线的偏移(正=多消费拉低水位,负=少消费抬高水位)。
/// **软死区**:水位误差在 ±deadband 内时视为 0,正常回调抖动不触发任何响应,trim 平滑归零
/// → 恢复逐样本精确直通(step=1、插值系数恒 0)。只有累积漂移穿出死区才介入,且只用超出
/// 死区的部分驱动,保证边界连续无跳变。**PI**:纯 P 有稳态误差(维持 2% trim 需水位持续偏离
/// 上千样本、物理上买不到 → 抽干欠载),积分项在误差归零时保持 trim。**抗饱和**:钳位时不累加积分。
struct RateController {
    setpoint: f64,
    deadband: f64,
    trim: f64,
    integ: f64,
    max_trim: f64,
    kp: f64,
    ki: f64,
    clamped: bool,
}

impl RateController {
    fn new(setpoint: f64, deadband: f64) -> Self {
        Self {
            setpoint,
            deadband,
            trim: 0.0,
            integ: 0.0,
            max_trim: 0.03,
            // 慢环增益,避免可闻 pitch wobble。
            kp: 1.5e-5,
            ki: 6.0e-7,
            clamped: false,
        }
    }

    /// 用「本次消费后预测水位」更新并返回 trim。
    fn update(&mut self, projected_level: f64) -> f64 {
        let error = projected_level - self.setpoint;
        let effective_error = if error > self.deadband {
            error - self.deadband
        } else if error < -self.deadband {
            error + self.deadband
        } else {
            0.0
        };
        if effective_error == 0.0 {
            // 死区内:积分与 trim 平滑归零;足够小则 snap 到严格 0,保证精确直通。
            self.integ *= 0.9;
            self.trim *= 0.9;
            if self.trim.abs() < 1.0e-6 {
                self.trim = 0.0;
                self.integ = 0.0;
            }
            self.clamped = false;
        } else {
            let unclamped = self.kp * effective_error + self.integ + self.ki * effective_error;
            let clamped_trim = unclamped.clamp(-self.max_trim, self.max_trim);
            self.clamped = unclamped != clamped_trim;
            if !self.clamped {
                self.integ += self.ki * effective_error;
            }
            self.trim = clamped_trim;
        }
        self.trim
    }
}

/// T3 输出侧自适应速率匹配。
///
/// 生产端锁 mic 时钟(处理循环每帧恒 push frame_size),消费端是设备/虚拟端点时钟。
/// 二者节奏不一致时 out_ring 水位会单调漂移 → 周期性 underrun(听感断续)。本重采样器
/// 在**输出回调线程**里读 out_ring 占用量,经 [`RateController`] 把有效消费比率朝设定点微调:
/// 水位偏高→多消费(step 略增),水位偏低→少消费(step 略减)。有效 trim 钳位 ±3%,
/// 稳态漂移(实测 Voicemeeter 残留 2.2%)被完全吸收且不净变调;配置级大错配(如 22%)
/// 会钳到边界后自然 underrun,交由 T1 的 clock_skew 检测告警,不硬追以免明显变调。
/// 正常设备(仅 ppm 漂移)几乎一直落在软死区内,trim=0 = 精确直通。
///
/// 线性插值 + 持久 `pos`/`buffer` 保证跨回调相位连续。稳态无堆分配(buffer 容量收敛),
/// 满足实时线程约束。
pub(super) struct AdaptiveOutputResampler {
    base_ratio: f64,
    controller: RateController,
    pos: f64,
    buffer: VecDeque<f32>,
}

impl AdaptiveOutputResampler {
    /// `in_rate`/`out_rate` 给出固定比率基线;`setpoint_samples` 是希望 out_ring 稳定的水位
    /// (通常 = 预填样本数);`deadband_samples` 是软死区半宽(误差在此内不介入 = 精确直通)。
    pub(super) fn new(
        in_rate: u32,
        out_rate: u32,
        setpoint_samples: usize,
        deadband_samples: usize,
    ) -> Self {
        let base_ratio = if out_rate == 0 {
            1.0
        } else {
            in_rate as f64 / out_rate as f64
        };
        Self {
            base_ratio,
            controller: RateController::new(setpoint_samples as f64, deadband_samples as f64),
            pos: 0.0,
            buffer: VecDeque::new(),
        }
    }

    /// 当前 trim(供诊断/测试断言);正=多消费,负=少消费。
    #[cfg(test)]
    pub(super) fn trim(&self) -> f64 {
        self.controller.trim
    }

    /// 上次填充是否触发钳位(= 水位反馈要求的比率超出 ±max_trim,即配置级错配)。
    #[cfg(test)]
    pub(super) fn is_clamped(&self) -> bool {
        self.controller.clamped
    }

    /// 用当前水位 `occupied` 更新有效比率,然后连续插值填满 `output`。
    pub(super) fn fill<C>(
        &mut self,
        output: &mut [f32],
        occupied: usize,
        consumer: &mut C,
        underruns: &AtomicU64,
    ) where
        C: Consumer<Item = f32>,
    {
        // 预测本次回调消费后的水位(occupied - output.len()),交控制器求 trim。
        let projected = occupied as f64 - output.len() as f64;
        let trim = self.controller.update(projected);
        let step = (self.base_ratio * (1.0 + trim)).max(1.0e-6);

        for sample in output.iter_mut() {
            *sample = self.next_sample(step, consumer, underruns);
        }
    }

    fn next_sample<C>(&mut self, step: f64, consumer: &mut C, underruns: &AtomicU64) -> f32
    where
        C: Consumer<Item = f32>,
    {
        let needed = (self.pos.floor() as usize).saturating_add(2);
        while self.buffer.len() < needed {
            match consumer.try_pop() {
                Some(sample) => self.buffer.push_back(sample.clamp(-1.0, 1.0)),
                None => {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    return 0.0;
                }
            }
        }

        let i0 = self.pos.floor() as usize;
        let frac = (self.pos - i0 as f64) as f32;
        let a = self.buffer.get(i0).copied().unwrap_or(0.0);
        let b = self.buffer.get(i0 + 1).copied().unwrap_or(a);
        let sample = (a + (b - a) * frac).clamp(-1.0, 1.0);

        self.pos += step;
        let consumed = self.pos.floor() as usize;
        for _ in 0..consumed {
            let _ = self.buffer.pop_front();
        }
        self.pos -= consumed as f64;
        sample
    }
}

/// T3 参考侧连续重采样。
///
/// 修复前 ref 流靠 `skip_stale` **硬丢帧**控制积压:每次积压超阈值就直接 `skip` 掉一段样本,
/// 使参考时间轴出现周期性缺口。AEC 依赖 near/far 的连续对齐,ref 时间轴碎片化会直接毁掉
/// 对齐(实测 mic↔ref 互相关峰仅 0.14~0.27、lag 乱跳)。本重采样器改为**连续重采样**:
/// 按 ref ring 水位微调消费比率(±3% 钳位),每帧恒定产出 frame_size 个参考样本,吸收
/// far 时钟(与输出同源的 Voicemeeter 端点)相对 mic 时钟的漂移,而不撕裂时间轴。
///
/// 多声道交织处理:每声道独立持久 `pos`/`buffer`(共享同一 step,保证声道间样本对齐)。
pub(super) struct AdaptiveReferenceResampler {
    channels: usize,
    controller: RateController,
    pos: f64,
    buffers: Vec<VecDeque<f32>>,
}

impl AdaptiveReferenceResampler {
    /// `channels` = 参考声道数;`setpoint_frames` = 希望 ref ring 稳定的**帧**水位;
    /// `deadband_frames` = 软死区半宽(帧,误差在此内不介入 = 连续直通)。
    pub(super) fn new(channels: usize, setpoint_frames: usize, deadband_frames: usize) -> Self {
        let channels = channels.max(1);
        Self {
            channels,
            controller: RateController::new(setpoint_frames as f64, deadband_frames as f64),
            pos: 0.0,
            buffers: (0..channels).map(|_| VecDeque::new()).collect(),
        }
    }

    #[cfg(test)]
    pub(super) fn trim(&self) -> f64 {
        self.controller.trim
    }

    /// 从交织的 `consumer` 连续重采样出 `out_frames` 帧(交织写入 `out`,长度必须
    /// = out_frames*channels)。`occupied_frames` = 当前 ref ring 的**帧**占用量。
    /// 返回本次因参考欠载而填零的帧数(供 underrun 统计)。
    pub(super) fn fill<C>(
        &mut self,
        out: &mut [f32],
        out_frames: usize,
        occupied_frames: usize,
        consumer: &mut C,
    ) -> usize
    where
        C: Consumer<Item = f32>,
    {
        // 预测消费后帧水位,交控制器求 trim(含软死区:正常抖动 → trim 0 = 连续直通)。
        let projected = occupied_frames as f64 - out_frames as f64;
        let trim = self.controller.update(projected);
        let step = (1.0 + trim).max(1.0e-6);

        let mut underrun_frames = 0;
        for frame_index in 0..out_frames {
            let ok = self.next_frame(step, consumer, out, frame_index * self.channels);
            if !ok {
                underrun_frames += 1;
            }
        }
        underrun_frames
    }

    fn next_frame<C>(
        &mut self,
        step: f64,
        consumer: &mut C,
        out: &mut [f32],
        out_base: usize,
    ) -> bool
    where
        C: Consumer<Item = f32>,
    {
        let needed = (self.pos.floor() as usize).saturating_add(2);
        // 交织拉取:一次补齐所有声道到 needed 深度。任一声道拉不到即判欠载填零。
        while self.buffers[0].len() < needed {
            let mut got_full_frame = true;
            for ch in 0..self.channels {
                match consumer.try_pop() {
                    Some(v) => self.buffers[ch].push_back(v.clamp(-1.0, 1.0)),
                    None => {
                        got_full_frame = false;
                        break;
                    }
                }
            }
            if !got_full_frame {
                for sample in out.iter_mut().skip(out_base).take(self.channels) {
                    *sample = 0.0;
                }
                return false;
            }
        }

        let i0 = self.pos.floor() as usize;
        let frac = (self.pos - i0 as f64) as f32;
        for ch in 0..self.channels {
            let a = self.buffers[ch].get(i0).copied().unwrap_or(0.0);
            let b = self.buffers[ch].get(i0 + 1).copied().unwrap_or(a);
            out[out_base + ch] = (a + (b - a) * frac).clamp(-1.0, 1.0);
        }

        self.pos += step;
        let consumed = self.pos.floor() as usize;
        for _ in 0..consumed {
            for ch in 0..self.channels {
                let _ = self.buffers[ch].pop_front();
            }
        }
        self.pos -= consumed as f64;
        true
    }
}

fn resize_planes(planes: &mut Vec<Vec<f32>>, channels: usize, frames: usize) {
    if planes.len() != channels {
        planes.resize_with(channels, Vec::new);
    }
    for plane in planes.iter_mut() {
        plane.resize(frames, 0.0);
    }
}

fn scaled_frames(input_frames: usize, in_rate: u32, out_rate: u32) -> usize {
    if in_rate == 0 {
        return input_frames;
    }
    ((input_frames as f64) * out_rate as f64 / in_rate as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use ringbuf::traits::{Observer, Producer, Split};
    use ringbuf::HeapRb;

    use super::*;

    #[test]
    fn input_resampler_upsamples_and_reuses_buffers() {
        let mut resampler = InterleavedInputResampler::new(24_000, 48_000, 1);
        let input = sine_block(240, 440.0, 24_000);

        let first = resampler.process(&input).to_vec();
        let capacity = input_capacity_signature(&resampler);
        let second = resampler.process(&input).to_vec();

        assert!(first.len() >= 480);
        assert!(first.iter().all(|sample| sample.is_finite()));
        assert!(second.iter().all(|sample| sample.is_finite()));
        assert_eq!(input_capacity_signature(&resampler), capacity);
    }

    #[test]
    fn input_resampler_downsamples_stereo_without_collapsing_channels() {
        let mut resampler = InterleavedInputResampler::new(48_000, 24_000, 2);
        let mut input = vec![0.0; 480 * 2];
        for frame in 0..480 {
            input[frame * 2] = 0.5;
            input[frame * 2 + 1] = -0.5;
        }

        let output = resampler.process(&input);

        assert!(output.len() >= 240 * 2);
        let mut left_sum = 0.0f32;
        let mut right_sum = 0.0f32;
        let mut frames = 0usize;
        for frame in output.chunks_exact(2).skip(16).take(200) {
            left_sum += frame[0];
            right_sum += frame[1];
            frames += 1;
        }
        let left_avg = left_sum / frames as f32;
        let right_avg = right_sum / frames as f32;
        assert!(left_avg > 0.2, "left channel collapsed: {left_avg}");
        assert!(right_avg < -0.2, "right channel collapsed: {right_avg}");
    }

    #[test]
    fn output_resampler_returns_requested_device_chunk() {
        let drops = AtomicU64::new(0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(4096).split();
        let input = sine_block(960, 440.0, 48_000);
        assert_eq!(prod.push_slice(&input), input.len());
        let mut resampler = OutputDeviceResampler::new(48_000, 24_000);

        let output = resampler.next_chunk(240, &mut cons, &drops);

        assert_eq!(output.len(), 240);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn output_resampler_passthrough_pulls_pipeline_samples() {
        let drops = AtomicU64::new(0);
        let (mut prod, mut cons) = HeapRb::<f32>::new(8).split();
        assert_eq!(prod.push_slice(&[0.0, 0.25, 0.5, 0.75]), 4);
        let mut resampler = OutputDeviceResampler::new(48_000, 48_000);

        let output = resampler.next_chunk(4, &mut cons, &drops);

        assert_eq!(output, &[0.0, 0.25, 0.5, 0.75]);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
    }

    fn sine_block(frames: usize, hz: f32, rate: u32) -> Vec<f32> {
        use std::f32::consts::TAU;

        (0..frames)
            .map(|i| 0.2 * (i as f32 * hz * TAU / rate as f32).sin())
            .collect()
    }

    fn input_capacity_signature(resampler: &InterleavedInputResampler) -> Vec<usize> {
        let mut caps = vec![resampler.output.capacity(), resampler.planes.capacity()];
        caps.extend(resampler.planes.iter().map(Vec::capacity));
        caps.push(resampler.resampled_planes.capacity());
        caps.extend(resampler.resampled_planes.iter().map(Vec::capacity));
        caps
    }

    // ── T3 输出侧自适应速率匹配回归 ──────────────────────────────────────────────
    //
    // 模型:生产端锁 mic 时钟,每 tick 恰好 push `frame_size` 个样本。消费端锁设备时钟,
    // 设备比 mic 快 `skew` → 每 tick 索取 `round(frame_size*(1+skew))` 个样本。跑足够多
    // tick,观察稳态 underrun 与 out_ring 水位是否被 trim 吸收。

    fn run_output_skew(skew: f64, frame_size: usize, ticks: usize) -> (u64, f64, f64, usize) {
        let ring_cap = frame_size * 12;
        let setpoint = frame_size * 2;
        let deadband = frame_size / 2;
        let (mut prod, mut cons) = HeapRb::<f32>::new(ring_cap).split();
        let mut resampler = AdaptiveOutputResampler::new(48_000, 48_000, setpoint, deadband);
        let underruns = AtomicU64::new(0);
        // 预填到设定点。
        let preroll = vec![0.0f32; setpoint];
        prod.push_slice(&preroll);

        let device_frames = ((frame_size as f64) * (1.0 + skew)).round() as usize;
        let mut out = vec![0.0f32; device_frames];
        let mut phase = 0.0f32;
        let mut clamps = 0usize;
        let mut last_half_underruns = 0u64;
        let half = ticks / 2;

        for tick in 0..ticks {
            // 生产:一帧 440Hz 正弦(mic 时钟)。
            let mut block = vec![0.0f32; frame_size];
            for s in block.iter_mut() {
                *s = 0.2 * (phase * std::f32::consts::TAU).sin();
                phase += 440.0 / 48_000.0;
            }
            prod.push_slice(&block);

            // 消费:设备索取 device_frames。
            let occupied = cons.occupied_len();
            resampler.fill(&mut out, occupied, &mut cons, &underruns);
            if resampler.is_clamped() {
                clamps += 1;
            }
            if tick == half {
                last_half_underruns = underruns.load(Ordering::Relaxed);
            }
        }

        let total = underruns.load(Ordering::Relaxed);
        let steady_underruns = total - last_half_underruns;
        let final_trim = resampler.trim();
        let clamp_ratio = clamps as f64 / ticks as f64;
        (
            steady_underruns,
            final_trim,
            clamp_ratio,
            steady_underruns as usize,
        )
    }

    #[test]
    fn t3_absorbs_small_skew_without_steady_underrun() {
        // 明确在 ±3% 权限内的漂移(实测 Voicemeeter 残留 2.2%):trim 收敛到 1/(1+skew)-1,
        // 稳态(后半程)underrun≈0,基本不钳位。
        for skew in [0.005_f64, 0.01, 0.022] {
            let expected_trim = 1.0 / (1.0 + skew) - 1.0;
            let (steady, trim, clamp_ratio, _) = run_output_skew(skew, 480, 2000);
            assert!(
                steady <= 480,
                "skew={skew}: steady underruns {steady} 应≈0(被 trim 吸收)"
            );
            assert!(
                (trim - expected_trim).abs() < 0.01,
                "skew={skew}: trim {trim:.4} 应收敛到 {expected_trim:.4}"
            );
            assert!(
                clamp_ratio < 0.05,
                "skew={skew}: clamp_ratio {clamp_ratio:.3} 应基本不钳位"
            );
        }
    }

    #[test]
    fn t3_boundary_skew_still_converges() {
        // 3% 恰在 ±3% 权限边界:收敛瞬态可能短暂钳位,但稳态 underrun 仍≈0、trim 贴边界。
        let skew = 0.03_f64;
        let (steady, trim, _clamp, _) = run_output_skew(skew, 480, 4000);
        assert!(steady <= 480, "边界 skew=3%: 稳态 underrun {steady} 应≈0");
        assert!(
            trim < -0.025,
            "边界 skew=3%: trim {trim:.4} 应贴近 -3% 权限"
        );
    }

    #[test]
    fn t3_clamps_on_config_level_skew() {
        // 8.8% / 22% 配置级错配:设备快到需要的拉伸超出 -3% 钳位边界,trim 钳到 -3%,
        // 不硬追(避免变调),后半程持续钳位(交由 T1 clock_skew 告警)。
        for skew in [0.088_f64, 0.224] {
            let (_steady, trim, clamp_ratio, _) = run_output_skew(skew, 480, 2000);
            assert!(
                (trim + 0.03).abs() < 0.005,
                "skew={skew}: trim {trim:.4} 应钳在 -3% 边界"
            );
            assert!(
                clamp_ratio > 0.5,
                "skew={skew}: clamp_ratio {clamp_ratio:.3} 应长期钳位(触发 T1 告警域)"
            );
        }
    }

    #[test]
    fn t3_no_skew_is_transparent_passthrough() {
        // 零漂移:水位恒在设定点(误差落死区内),trim 严格归零 → 逐样本精确直通,无 underrun。
        let (steady, trim, clamp_ratio, _) = run_output_skew(0.0, 480, 1000);
        assert_eq!(steady, 0, "零漂移不应有稳态 underrun");
        assert_eq!(trim, 0.0, "零漂移 trim 应严格为 0(死区内),实际 {trim:.9}");
        assert!(clamp_ratio < 0.01);
    }

    #[test]
    fn t3_deadband_holds_trim_at_zero_under_normal_jitter() {
        // 正常设备:无净漂移,但回调帧数抖动(WASAPI GetCurrentPadding)。抖动幅度远小于半帧
        // 死区,控制器不应介入 → trim 严格保持 0 = 精确直通,不引入 pitch wobble。
        let frame_size = 480;
        let ring_cap = frame_size * 12;
        let setpoint = frame_size * 2;
        let deadband = frame_size / 2;
        let (mut prod, mut cons) = HeapRb::<f32>::new(ring_cap).split();
        let mut resampler = AdaptiveOutputResampler::new(48_000, 48_000, setpoint, deadband);
        let underruns = AtomicU64::new(0);
        prod.push_slice(&vec![0.0f32; setpoint]);

        let mut phase = 0.0f32;
        let mut max_abs_trim = 0.0f64;
        for tick in 0..1000 {
            let mut block = vec![0.0f32; frame_size];
            for s in block.iter_mut() {
                *s = 0.2 * (phase * std::f32::consts::TAU).sin();
                phase += 440.0 / 48_000.0;
            }
            prod.push_slice(&block);
            // 回调帧数在 ±16 样本抖动(无净漂移),远小于半帧(240)死区。
            let jitter = if tick % 2 == 0 { 16 } else { -16 };
            let device_frames = (frame_size as isize + jitter) as usize;
            let mut out = vec![0.0f32; device_frames];
            let occupied = cons.occupied_len();
            resampler.fill(&mut out, occupied, &mut cons, &underruns);
            max_abs_trim = max_abs_trim.max(resampler.trim().abs());
        }
        assert_eq!(underruns.load(Ordering::Relaxed), 0, "正常抖动不应欠载");
        assert_eq!(
            max_abs_trim, 0.0,
            "死区内 trim 应始终为 0,实际峰值 {max_abs_trim:.9}"
        );
    }

    #[test]
    fn t3_reference_resampler_keeps_continuity_no_hard_drops() {
        // 参考侧:mono 连续重采样,far 时钟快 2.2% 时稳态不欠载,trim 收敛,输出连续有限值。
        let frame_size = 480;
        let ring_cap = frame_size * 12;
        let (mut prod, mut cons) = HeapRb::<f32>::new(ring_cap).split();
        let mut resampler = AdaptiveReferenceResampler::new(1, frame_size * 2, frame_size / 2);
        prod.push_slice(&vec![0.0f32; frame_size * 2]);

        let skew = 0.022_f64;
        let device_frames = ((frame_size as f64) * (1.0 + skew)).round() as usize;
        let mut far = vec![0.0f32; device_frames];
        let mut phase = 0.0f32;
        let mut underruns_second_half = 0usize;
        let ticks = 2000;

        for tick in 0..ticks {
            let mut block = vec![0.0f32; frame_size];
            for s in block.iter_mut() {
                *s = 0.2 * (phase * std::f32::consts::TAU).sin();
                phase += 440.0 / 48_000.0;
            }
            prod.push_slice(&block);
            let occ_frames = cons.occupied_len();
            let u = resampler.fill(&mut far, device_frames, occ_frames, &mut cons);
            if tick >= ticks / 2 {
                underruns_second_half += u;
            }
            assert!(far.iter().all(|s| s.is_finite()));
        }
        assert!(
            underruns_second_half <= device_frames,
            "参考侧稳态欠载 {underruns_second_half} 应≈0(连续重采样吸收漂移)"
        );
        let expected_trim = 1.0 / (1.0 + skew) - 1.0;
        assert!(
            (resampler.trim() - expected_trim).abs() < 0.01,
            "参考侧 trim {:.4} 应收敛到 {expected_trim:.4}",
            resampler.trim()
        );
    }
}
