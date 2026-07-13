use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use echoless_processors::{
    rnnoise::RnNoise, webrtc_ns::WebRtcNs, EchoProcessor, IoSpec, ProcessorChain, ProcessorStats,
};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static LAST_ALLOCATION_SIZE: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        LAST_ALLOCATION_SIZE.store(layout.size(), Ordering::SeqCst);
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        LAST_ALLOCATION_SIZE.store(layout.size(), Ordering::SeqCst);
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        LAST_ALLOCATION_SIZE.store(new_size, Ordering::SeqCst);
        System.realloc(ptr, layout, new_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

struct Identity16k;

impl EchoProcessor for Identity16k {
    fn name(&self) -> &'static str {
        "identity_16k"
    }

    fn io_spec(&self) -> IoSpec {
        IoSpec {
            sample_rate: 16_000,
            near_channels: 1,
            far_channels: 1,
            algorithmic_latency_ms: 0.0,
        }
    }

    fn configure(&mut self, _params: &toml::Table) -> anyhow::Result<()> {
        Ok(())
    }

    fn process(&mut self, near: &[f32], _far: &[f32], out: &mut [f32], _frames: u32) {
        let n = near.len().min(out.len());
        out[..n].copy_from_slice(&near[..n]);
        out[n..].fill(0.0);
    }

    fn stats(&self) -> ProcessorStats {
        ProcessorStats::empty("identity_16k")
    }

    fn reset(&mut self) {}
}

#[test]
fn processor_chain_process_is_allocation_free_after_warmup() {
    let near = sine_block(480, 440.0, 48_000);
    let far = vec![0.0; 480];
    let mut out = vec![0.0; 480];
    let mut chain = ProcessorChain::new(48_000, 1);
    chain.push(Box::new(Identity16k));

    for _ in 0..4 {
        chain.process(&near, &far, &mut out, 480);
    }

    ALLOCATIONS.store(0, Ordering::SeqCst);
    chain.process(&near, &far, &mut out, 480);

    assert_eq!(ALLOCATIONS.load(Ordering::SeqCst), 0);

    let mut ns = WebRtcNs::new();
    for _ in 0..4 {
        ns.process(&near, &far, &mut out, 480);
    }

    ALLOCATIONS.store(0, Ordering::SeqCst);
    ns.process(&near, &far, &mut out, 480);

    assert_eq!(
        ALLOCATIONS.load(Ordering::SeqCst),
        0,
        "standalone WebRTC NS allocated after warmup (last size: {})",
        LAST_ALLOCATION_SIZE.load(Ordering::SeqCst)
    );

    let mut ns_chain = ProcessorChain::new(48_000, 1);
    ns_chain.push(Box::new(ns));
    for _ in 0..4 {
        ns_chain.process(&near, &far, &mut out, 480);
    }

    ALLOCATIONS.store(0, Ordering::SeqCst);
    ns_chain.process(&near, &far, &mut out, 480);

    assert_eq!(
        ALLOCATIONS.load(Ordering::SeqCst),
        0,
        "WebRTC NS chain allocated after warmup"
    );

    let mut rnnoise_chain = ProcessorChain::new(48_000, 1);
    rnnoise_chain.push(Box::new(RnNoise::try_new().unwrap()));
    let allocations = std::thread::spawn(move || {
        let near = sine_block(480, 440.0, 48_000);
        let far = vec![0.0; 480];
        let mut out = vec![0.0; 480];
        rnnoise_chain.warm_up(480);

        ALLOCATIONS.store(0, Ordering::SeqCst);
        rnnoise_chain.process(&near, &far, &mut out, 480);
        ALLOCATIONS.load(Ordering::SeqCst)
    })
    .join()
    .unwrap();

    assert_eq!(
        allocations, 0,
        "RNNoise allocated after audio-thread warmup"
    );
}

fn sine_block(frames: usize, hz: f32, sample_rate: u32) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let phase = frame as f32 * hz * std::f32::consts::TAU / sample_rate as f32;
            0.1 * phase.sin()
        })
        .collect()
}
