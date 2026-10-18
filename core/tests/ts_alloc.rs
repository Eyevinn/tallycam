//! The muxer's per-frame path allocates nothing once its buffers have grown
//! to the largest frame (the core's rule for calls made per frame).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tallycam_core::sei::Fields;
use tallycam_core::ts::{AudioCodec, Muxer, VideoCodec, VideoFrame};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn no_allocation_per_frame_after_warm_up() {
    // nal_length_size 4 (VideoToolbox) and 0 (Annex B, MediaCodec).
    for (video, audio, nal_length_size) in [
        (VideoCodec::H264, AudioCodec::Opus, 4),
        (VideoCodec::Hevc, AudioCodec::Aac, 4),
        (VideoCodec::H264, AudioCodec::Opus, 0),
        (VideoCodec::Hevc, AudioCodec::Aac, 0),
    ] {
        let mut m = Muxer::new(video, Some(audio), AudioCodec::pcr_delay_ns(Some(audio)));
        match video {
            VideoCodec::H264 => m
                .set_video_format([&[0x67, 1, 2][..], &[0x68, 3][..]], nal_length_size)
                .unwrap(),
            VideoCodec::Hevc => m
                .set_video_format(
                    [&[0x40, 1][..], &[0x42, 1][..], &[0x44, 1][..]],
                    nal_length_size,
                )
                .unwrap(),
        }
        m.set_audio_format(48_000, 1).unwrap();
        // A 600 kB keyframe (more than the initial capacity) and small frames.
        let mut key = vec![0u8; 600_000];
        if nal_length_size == 0 {
            // Many slices (more NAL units than the muxer's first capacity).
            for i in (0..600_000).step_by(1000) {
                key[i..i + 5].copy_from_slice(&[0, 0, 0, 1, key_type(video)]);
            }
        } else {
            key[..4].copy_from_slice(&(600_000u32 - 4).to_be_bytes());
        }
        key[4] = key_type(video);
        let mut p = vec![0x5Au8; 20_000];
        if nal_length_size == 0 {
            p[..4].copy_from_slice(&[0, 0, 0, 1]);
        } else {
            p[..4].copy_from_slice(&(20_000u32 - 4).to_be_bytes());
        }
        p[4] = if video == VideoCodec::H264 {
            0x41
        } else {
            0x02
        };
        let audio_frame = [0x11u8; 300];
        let mut bytes = 0usize;
        let mut run = |m: &mut Muxer, from: i64| {
            for i in from..from + 60 {
                let t = i * 33_333_333;
                let frame = VideoFrame {
                    data: if i % 30 == 0 { &key } else { &p },
                    keyframe: i % 30 == 0,
                    pts_ns: t,
                    sei: Some(Fields {
                        capture_ns: t as u64,
                        synced: true,
                        tai: true,
                        frame_counter: i as u32,
                        uncertainty_us: 1,
                    }),
                };
                m.write_video(&frame, t + 30_000_000, &mut |ts, _| bytes += ts.len());
                m.write_audio(&audio_frame, t, t + 20_000_000, &mut |ts, _| {
                    bytes += ts.len()
                });
            }
        };
        run(&mut m, 0);
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        run(&mut m, 60);
        assert_eq!(
            ALLOCATIONS.load(Ordering::Relaxed) - before,
            0,
            "{video:?} {audio:?} {nal_length_size}"
        );
        assert!(bytes > 0);
    }
}

/// First byte of an IDR slice.
fn key_type(video: VideoCodec) -> u8 {
    if video == VideoCodec::H264 {
        0x65
    } else {
        0x26
    }
}
