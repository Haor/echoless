use std::sync::atomic::{AtomicU64, Ordering};

use ringbuf::traits::{Consumer, Producer};

/// Commit or reject one complete SPSC frame. The producer is the only side that
/// reduces vacant capacity, so per-sample pushes cannot fail after this preflight.
pub(super) fn try_push_frame_with<P>(
    producer: &mut P,
    channels: usize,
    mut sample_at: impl FnMut(usize) -> f32,
) -> bool
where
    P: Producer<Item = f32>,
{
    let channels = channels.max(1);
    if producer.vacant_len() < channels {
        return false;
    }
    for channel in 0..channels {
        let pushed = producer.try_push(sample_at(channel));
        debug_assert!(pushed.is_ok(), "preflighted frame push became partial");
    }
    true
}

pub(super) fn try_push_frame<P>(producer: &mut P, frame: &[f32]) -> bool
where
    P: Producer<Item = f32>,
{
    if frame.is_empty() {
        return true;
    }
    try_push_frame_with(producer, frame.len(), |channel| frame[channel])
}

/// Commit complete interleaved frames and count rejected frames, independent of
/// channel count.
pub(super) fn push_interleaved_frames<P>(
    producer: &mut P,
    samples: &[f32],
    channels: usize,
    drops: &AtomicU64,
) where
    P: Producer<Item = f32>,
{
    let channels = channels.max(1);
    let mut frames = samples.chunks_exact(channels);
    for frame in &mut frames {
        if !try_push_frame(producer, frame) {
            drops.fetch_add(1, Ordering::Relaxed);
        }
    }
    debug_assert!(
        frames.remainder().is_empty(),
        "interleaved input ended with a partial frame"
    );
}

/// Pop one complete frame. The consumer is the only side that reduces occupied
/// length, so an underflow consumes no channel and cannot strand channel zero.
pub(super) fn try_pop_frame<C>(consumer: &mut C, frame: &mut [f32]) -> bool
where
    C: Consumer<Item = f32>,
{
    if frame.is_empty() {
        return true;
    }
    if consumer.occupied_len() < frame.len() {
        return false;
    }
    let popped = consumer.pop_slice(frame);
    debug_assert_eq!(popped, frame.len(), "preflighted frame pop became partial");
    true
}

pub(super) fn skip_stale_aligned<C>(
    consumer: &mut C,
    frame_samples: usize,
    channels: usize,
) -> usize
where
    C: Consumer<Item = f32>,
{
    let channels = channels.max(1);
    let max_queued = frame_samples * 4;
    let queued = consumer.occupied_len();
    if queued <= max_queued {
        return 0;
    }
    let excess = queued - max_queued;
    let dropped = excess - excess % channels;
    consumer.skip(dropped);
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    use ringbuf::traits::{Observer, Split};
    use ringbuf::HeapRb;

    #[test]
    fn stereo_push_with_one_slot_left_drops_the_whole_frame() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(3).split();
        assert!(producer.try_push(10.0).is_ok());
        assert!(producer.try_push(11.0).is_ok());

        assert!(!try_push_frame(&mut producer, &[1.0, -1.0]));
        assert_eq!(consumer.occupied_len(), 2);
        let mut existing = [0.0; 2];
        assert!(try_pop_frame(&mut consumer, &mut existing));
        assert_eq!(existing, [10.0, 11.0]);
    }

    #[test]
    fn single_channel_fragment_is_not_consumed_as_a_stereo_frame() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(4).split();
        assert!(producer.try_push(1.0).is_ok());
        let mut frame = [7.0, 8.0];

        assert!(!try_pop_frame(&mut consumer, &mut frame));
        assert_eq!(frame, [7.0, 8.0]);
        assert_eq!(consumer.occupied_len(), 1);
    }

    #[test]
    fn stale_skip_never_discards_an_odd_stereo_sample_count() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(16).split();
        for sample in 0..13 {
            assert!(producer.try_push(sample as f32).is_ok());
        }

        let dropped = skip_stale_aligned(&mut consumer, 2, 2);

        assert_eq!(dropped, 4);
        assert_eq!(consumer.occupied_len(), 9);
    }

    #[test]
    fn mono_stale_skip_keeps_the_previous_sample_semantics() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(8).split();
        for sample in 0..7 {
            assert!(producer.try_push(sample as f32).is_ok());
        }

        assert_eq!(skip_stale_aligned(&mut consumer, 1, 1), 3);
        let remaining: Vec<f32> = std::iter::from_fn(|| consumer.try_pop()).collect();
        assert_eq!(remaining, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn concurrent_overflow_never_crosses_stereo_frame_boundaries() {
        const FRAMES: usize = 20_000;
        let (mut producer, mut consumer) = HeapRb::<f32>::new(4).split();
        let done = Arc::new(AtomicBool::new(false));
        let consumer_done = done.clone();
        let reader = thread::spawn(move || {
            let mut previous = -1.0f32;
            let mut consumed = 0usize;
            let mut frame = [0.0; 2];
            while !consumer_done.load(Ordering::Acquire) || consumer.occupied_len() > 0 {
                if try_pop_frame(&mut consumer, &mut frame) {
                    assert_eq!(frame[1], -frame[0]);
                    assert!(frame[0] > previous);
                    previous = frame[0];
                    consumed += 1;
                } else {
                    thread::yield_now();
                }
            }
            assert_eq!(consumer.occupied_len(), 0);
            consumed
        });

        let mut accepted = 0usize;
        let mut dropped = 0usize;
        for index in 0..FRAMES {
            let left = index as f32 + 1.0;
            if try_push_frame(&mut producer, &[left, -left]) {
                accepted += 1;
            } else {
                dropped += 1;
            }
            if index % 17 == 0 {
                thread::yield_now();
            }
        }
        done.store(true, Ordering::Release);

        assert_eq!(reader.join().unwrap(), accepted);
        assert_eq!(accepted + dropped, FRAMES);
        assert!(dropped > 0, "test did not exercise overflow");
    }
}
