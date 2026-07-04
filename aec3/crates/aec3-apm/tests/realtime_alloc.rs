use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use aec3_apm::config::{EchoCanceller, MaxProcessingRate, Pipeline};
use aec3_apm::{AudioProcessing, Config, StreamConfig};

struct CountingAllocator;

static COUNT_ALLOCATIONS: AtomicBool = AtomicBool::new(false);
static ALLOCATION_COUNT: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNT_ALLOCATIONS.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn run_without_counting_allocations<F>(f: F) -> usize
where
    F: FnOnce(),
{
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    COUNT_ALLOCATIONS.store(true, Ordering::SeqCst);
    f();
    COUNT_ALLOCATIONS.store(false, Ordering::SeqCst);
    ALLOCATION_COUNT.load(Ordering::Relaxed)
}

fn assert_aec_round_trip_is_allocation_free(render_channels: usize, capture_channels: usize) {
    let config = Config {
        echo_canceller: Some(EchoCanceller::default()),
        pipeline: Pipeline {
            maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
            multi_channel_render: render_channels > 1,
            multi_channel_capture: capture_channels > 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut apm = AudioProcessing::builder().config(config).build();

    let render_stream = StreamConfig::new(48_000, render_channels as u16);
    let capture_stream = StreamConfig::new(48_000, capture_channels as u16);
    let frames = render_stream.num_frames();

    let render_input: Vec<Vec<f32>> = (0..render_channels)
        .map(|ch| {
            (0..frames)
                .map(|i| ((i + ch) % 17) as f32 * 0.001)
                .collect()
        })
        .collect();
    let mut render_output = vec![vec![0.0f32; frames]; render_channels];
    let render_src: Vec<&[f32]> = render_input.iter().map(Vec::as_slice).collect();
    let mut render_dest: Vec<&mut [f32]> =
        render_output.iter_mut().map(Vec::as_mut_slice).collect();

    let capture_input: Vec<Vec<f32>> = (0..capture_channels)
        .map(|ch| {
            (0..frames)
                .map(|i| ((i + ch * 3) % 23) as f32 * 0.001)
                .collect()
        })
        .collect();
    let mut capture_output = vec![vec![0.0f32; frames]; capture_channels];
    let capture_src: Vec<&[f32]> = capture_input.iter().map(Vec::as_slice).collect();
    let mut capture_dest: Vec<&mut [f32]> =
        capture_output.iter_mut().map(Vec::as_mut_slice).collect();

    for _ in 0..120 {
        apm.process_render_f32_with_config(
            &render_src,
            &render_stream,
            &render_stream,
            &mut render_dest,
        )
        .unwrap();
        apm.process_capture_f32_with_config(
            &capture_src,
            &capture_stream,
            &capture_stream,
            &mut capture_dest,
        )
        .unwrap();
    }

    let allocations = run_without_counting_allocations(|| {
        for _ in 0..20 {
            apm.process_render_f32_with_config(
                &render_src,
                &render_stream,
                &render_stream,
                &mut render_dest,
            )
            .unwrap();
            apm.process_capture_f32_with_config(
                &capture_src,
                &capture_stream,
                &capture_stream,
                &mut capture_dest,
            )
            .unwrap();
        }
    });

    assert_eq!(
        allocations, 0,
        "steady-state AEC3 render/capture allocated {allocations} times for render_channels={render_channels}, capture_channels={capture_channels}",
    );
}

#[test]
fn aec3_mono_render_capture_is_allocation_free_after_warmup() {
    assert_aec_round_trip_is_allocation_free(1, 1);
}

#[test]
fn aec3_stereo_render_mono_capture_is_allocation_free_after_warmup() {
    assert_aec_round_trip_is_allocation_free(2, 1);
}
