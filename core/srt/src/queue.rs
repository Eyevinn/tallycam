//! Single-producer single-consumer queue of SRT payloads between the
//! encoder/muxer thread and the SRT thread.
//!
//! Fixed slots allocated once; a push never locks. It copies each payload
//! once, into a shared chunk ([`PayloadArena`]: one allocation per 64
//! payloads), and publishes it with one atomic store. The SRT thread hands
//! the payload to the engine as it is, without copying it again.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bytes::{Bytes, BytesMut};

use crate::config::PAYLOAD_SIZE;

/// The payload starts a video frame: report when it first goes out.
pub const MARK_FRAME_START: u8 = 0x01;
/// The payload ends a video frame.
pub const MARK_FRAME_END: u8 = 0x02;

pub struct Slot {
    pub marks: u8,
    /// Capture time of the frame this payload starts or ends (host ns).
    pub capture_ns: i64,
    /// When the payload was queued (host ns).
    pub enqueued_ns: i64,
    pub data: Bytes,
}

/// Hands out payloads as `Bytes` slices of shared chunks: one allocation
/// per [`ARENA_PAYLOADS`] payloads instead of one per payload. A chunk is
/// freed once every payload in it is dropped (the engine dropped it after
/// the ACK, and its slot was reused).
struct PayloadArena {
    chunk: BytesMut,
}

/// Payloads per arena chunk (about 84 kB).
const ARENA_PAYLOADS: usize = 64;

impl PayloadArena {
    fn payload(&mut self, data: &[u8]) -> Bytes {
        if self.chunk.capacity() < data.len() {
            self.chunk = BytesMut::with_capacity(ARENA_PAYLOADS * PAYLOAD_SIZE);
        }
        self.chunk.extend_from_slice(data);
        self.chunk.split().freeze()
    }
}

/// Keeps `head` and `tail` on cache lines of their own: the producer
/// writes one and the consumer the other, on different cores.
#[repr(align(128))]
struct Padded<T>(T);

pub struct SpscQueue {
    slots: Box<[UnsafeCell<Slot>]>,
    /// Next slot the producer writes; only the producer stores it.
    head: Padded<AtomicUsize>,
    /// Next slot the consumer reads; only the consumer stores it.
    tail: Padded<AtomicUsize>,
    /// Guards against a second producer (a caller bug), without blocking.
    producing: AtomicBool,
    /// Producer only (under `producing`).
    arena: UnsafeCell<PayloadArena>,
}

// SAFETY: a slot is written only by the producer between `tail` and `head`
// (exclusive, checked) and read only by the consumer after the producer
// published it with a release store of `head`. The arena is used only by
// the producer, under the `producing` flag.
unsafe impl Sync for SpscQueue {}
unsafe impl Send for SpscQueue {}

#[derive(Debug, PartialEq, Eq)]
pub enum PushError {
    /// Not enough free slots for the whole buffer; nothing was queued.
    Full,
    /// Another thread is pushing at the same time.
    Busy,
}

impl SpscQueue {
    pub fn new(capacity: usize) -> Self {
        let slots = (0..capacity.next_power_of_two())
            .map(|_| {
                UnsafeCell::new(Slot {
                    marks: 0,
                    capture_ns: 0,
                    enqueued_ns: 0,
                    data: Bytes::new(),
                })
            })
            .collect();
        SpscQueue {
            slots,
            head: Padded(AtomicUsize::new(0)),
            tail: Padded(AtomicUsize::new(0)),
            producing: AtomicBool::new(false),
            arena: UnsafeCell::new(PayloadArena {
                chunk: BytesMut::new(),
            }),
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.head.0.load(Ordering::Acquire) == self.tail.0.load(Ordering::Acquire)
    }

    /// Splits `data` into payloads of up to [`PAYLOAD_SIZE`] bytes and queues
    /// them all, or none. With `frame_capture_ns` != 0 the first and last
    /// payload carry the frame marks. Returns the number of payloads.
    pub fn push(
        &self,
        data: &[u8],
        frame_capture_ns: i64,
        now_ns: i64,
    ) -> Result<usize, PushError> {
        if self.producing.swap(true, Ordering::Acquire) {
            return Err(PushError::Busy);
        }
        let n = data.len().div_ceil(PAYLOAD_SIZE);
        let head = self.head.0.load(Ordering::Relaxed);
        let tail = self.tail.0.load(Ordering::Acquire);
        if head.wrapping_sub(tail) + n > self.slots.len() {
            self.producing.store(false, Ordering::Release);
            return Err(PushError::Full);
        }
        // SAFETY: only one producer runs (the `producing` flag).
        let arena = unsafe { &mut *self.arena.get() };
        for (i, chunk) in data.chunks(PAYLOAD_SIZE).enumerate() {
            // SAFETY: slots in [tail, tail + len) belong to the consumer;
            // head + i is outside them (checked above), and only one
            // producer runs (the `producing` flag).
            let slot = unsafe { &mut *self.slots[(head + i) & self.mask()].get() };
            slot.data = arena.payload(chunk);
            let mut marks = 0;
            if frame_capture_ns != 0 {
                if i == 0 {
                    marks |= MARK_FRAME_START;
                }
                if i + 1 == n {
                    marks |= MARK_FRAME_END;
                }
            }
            slot.marks = marks;
            slot.capture_ns = frame_capture_ns;
            slot.enqueued_ns = now_ns;
        }
        self.head.0.store(head.wrapping_add(n), Ordering::Release);
        self.producing.store(false, Ordering::Release);
        Ok(n)
    }

    /// Consumer: the oldest queued slot, if any; call [`Self::pop`] after use.
    pub fn front(&self) -> Option<&Slot> {
        let tail = self.tail.0.load(Ordering::Relaxed);
        if tail == self.head.0.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: published by the producer (acquire above), and the producer
        // does not touch it until `pop` moves `tail` past it.
        Some(unsafe { &*self.slots[tail & self.mask()].get() })
    }

    pub fn pop(&self) {
        let tail = self.tail.0.load(Ordering::Relaxed);
        self.tail.0.store(tail.wrapping_add(1), Ordering::Release);
    }

    /// Consumer: drops everything queued; returns how many payloads.
    pub fn clear(&self) -> usize {
        let head = self.head.0.load(Ordering::Acquire);
        let tail = self.tail.0.load(Ordering::Relaxed);
        self.tail.0.store(head, Ordering::Release);
        head.wrapping_sub(tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_marks_and_wraps() {
        let q = SpscQueue::new(4);
        let data: Vec<u8> = (0..(PAYLOAD_SIZE * 2 + 188)).map(|i| i as u8).collect();
        assert_eq!(q.push(&data, 77, 5), Ok(3));
        assert_eq!(q.push(&data, 0, 5), Err(PushError::Full));
        let s = q.front().unwrap();
        assert_eq!(
            (s.data.len(), s.marks, s.capture_ns),
            (PAYLOAD_SIZE, MARK_FRAME_START, 77)
        );
        assert_eq!(s.data[..], data[..PAYLOAD_SIZE]);
        q.pop();
        assert_eq!(q.front().unwrap().marks, 0);
        q.pop();
        let s = q.front().unwrap();
        assert_eq!((s.data.len(), s.marks), (188, MARK_FRAME_END));
        assert_eq!(s.data[..], data[PAYLOAD_SIZE * 2..]);
        q.pop();
        assert!(q.front().is_none());
        // Wraps around the end of the slots.
        assert_eq!(q.push(&data[..PAYLOAD_SIZE * 2], 0, 0), Ok(2));
        assert_eq!(q.push(&data[..100], 9, 0), Ok(1));
        assert_eq!(q.clear(), 3);
        assert!(q.is_empty());
        assert_eq!(q.push(&data[..1], 9, 0), Ok(1));
        assert_eq!(q.front().unwrap().marks, MARK_FRAME_START | MARK_FRAME_END);
    }

    #[test]
    fn payloads_share_arena_chunks() {
        let q = SpscQueue::new(256);
        let data: Vec<u8> = (0..PAYLOAD_SIZE * (ARENA_PAYLOADS + 1))
            .map(|i| (i / PAYLOAD_SIZE) as u8)
            .collect();
        assert_eq!(q.push(&data, 0, 0), Ok(ARENA_PAYLOADS + 1));
        let mut ptrs = Vec::new();
        for i in 0..=ARENA_PAYLOADS {
            let s = q.front().unwrap();
            assert!(s.data.iter().all(|&b| b == i as u8));
            ptrs.push(s.data.as_ptr() as usize);
            q.pop();
        }
        // Adjacent in one chunk, then a new chunk.
        assert_eq!(ptrs[1] - ptrs[0], PAYLOAD_SIZE);
        assert_eq!(
            ptrs[ARENA_PAYLOADS - 1] - ptrs[0],
            (ARENA_PAYLOADS - 1) * PAYLOAD_SIZE
        );
        assert_ne!(
            ptrs[ARENA_PAYLOADS],
            ptrs[ARENA_PAYLOADS - 1] + PAYLOAD_SIZE
        );
    }

    #[test]
    fn threads() {
        let q = std::sync::Arc::new(SpscQueue::new(64));
        let p = q.clone();
        let t = std::thread::spawn(move || {
            let mut sent = 0u32;
            while sent < 100_000 {
                let b = sent.to_le_bytes();
                if p.push(&b, 0, 0).is_ok() {
                    sent += 1;
                }
            }
        });
        let mut next = 0u32;
        while next < 100_000 {
            if let Some(s) = q.front() {
                assert_eq!(u32::from_le_bytes(s.data[..4].try_into().unwrap()), next);
                q.pop();
                next += 1;
            }
        }
        t.join().unwrap();
    }
}
