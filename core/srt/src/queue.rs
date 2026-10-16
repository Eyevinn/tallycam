//! Single-producer single-consumer queue of SRT payloads between the
//! encoder/muxer thread and the SRT thread.
//!
//! Fixed slots allocated once, so a push never allocates or locks: it
//! copies the bytes into a slot and publishes it with one atomic store.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::config::PAYLOAD_SIZE;

/// The payload starts a video frame: report when it first goes out.
pub const MARK_FRAME_START: u8 = 0x01;
/// The payload ends a video frame.
pub const MARK_FRAME_END: u8 = 0x02;

pub struct Slot {
    pub len: u16,
    pub marks: u8,
    /// Capture time of the frame this payload starts or ends (host ns).
    pub capture_ns: i64,
    /// When the payload was queued (host ns).
    pub enqueued_ns: i64,
    pub data: [u8; PAYLOAD_SIZE],
}

pub struct SpscQueue {
    slots: Box<[UnsafeCell<Slot>]>,
    /// Next slot the producer writes; only the producer stores it.
    head: AtomicUsize,
    /// Next slot the consumer reads; only the consumer stores it.
    tail: AtomicUsize,
    /// Guards against a second producer (a caller bug), without blocking.
    producing: AtomicBool,
}

// SAFETY: a slot is written only by the producer between `tail` and `head`
// (exclusive, checked) and read only by the consumer after the producer
// published it with a release store of `head`.
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
                    len: 0,
                    marks: 0,
                    capture_ns: 0,
                    enqueued_ns: 0,
                    data: [0; PAYLOAD_SIZE],
                })
            })
            .collect();
        SpscQueue {
            slots,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producing: AtomicBool::new(false),
        }
    }

    fn mask(&self) -> usize {
        self.slots.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
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
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) + n > self.slots.len() {
            self.producing.store(false, Ordering::Release);
            return Err(PushError::Full);
        }
        for (i, chunk) in data.chunks(PAYLOAD_SIZE).enumerate() {
            // SAFETY: slots in [tail, tail + len) belong to the consumer;
            // head + i is outside them (checked above), and only one
            // producer runs (the `producing` flag).
            let slot = unsafe { &mut *self.slots[(head + i) & self.mask()].get() };
            slot.len = chunk.len() as u16;
            slot.data[..chunk.len()].copy_from_slice(chunk);
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
        self.head.store(head.wrapping_add(n), Ordering::Release);
        self.producing.store(false, Ordering::Release);
        Ok(n)
    }

    /// Consumer: the oldest queued slot, if any; call [`Self::pop`] after use.
    pub fn front(&self) -> Option<&Slot> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: published by the producer (acquire above), and the producer
        // does not touch it until `pop` moves `tail` past it.
        Some(unsafe { &*self.slots[tail & self.mask()].get() })
    }

    pub fn pop(&self) {
        let tail = self.tail.load(Ordering::Relaxed);
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
    }

    /// Consumer: drops everything queued; returns how many payloads.
    pub fn clear(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);
        self.tail.store(head, Ordering::Release);
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
            (s.len as usize, s.marks, s.capture_ns),
            (PAYLOAD_SIZE, MARK_FRAME_START, 77)
        );
        q.pop();
        assert_eq!(q.front().unwrap().marks, 0);
        q.pop();
        let s = q.front().unwrap();
        assert_eq!((s.len, s.marks), (188, MARK_FRAME_END));
        assert_eq!(s.data[..188], data[PAYLOAD_SIZE * 2..]);
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
