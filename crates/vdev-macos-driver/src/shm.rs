//! The shared memory segment between the app (writer) and the driver
//! (reader): its name, its fixed layout, the ring protocol, and the code that
//! creates and maps it. Both sides use this module, so the layout has one
//! definition.
//!
//! The app creates the segment at every start, replacing any previous one,
//! with a new random generation. The driver runs as `_coreaudiod`, so the
//! segment is readable by every user (mode 0644): fine for the spike, not
//! for a release, where the app hands the segment over through an
//! authenticated channel instead.
//!
//! # Ring
//!
//! Audio is a bounded history indexed by absolute position on the device
//! timeline (see [`crate::clock`]): frame `p` lives in slot
//! `(p / SLOT_FRAMES) % SLOTS`. Each slot records which positions it holds
//! and a sequence number. The writer makes the sequence odd while it changes
//! a slot and even when done; the reader copies a slot only when it sees the
//! same even sequence before and after the copy, and the positions it wants.
//! Anything else reads as missing and becomes silence. The driver writes
//! nothing back, so it maps the segment read-only and never waits.
//!
//! Every shared field and sample is an atomic, so a racing copy is never a
//! data race. The reader uses relaxed loads with acquire fences, the access
//! Rust defines for read-only memory.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::*, fence};

/// POSIX shm name, within PSHMNAMLEN (31) on macOS.
pub const NAME: &std::ffi::CStr = c"/voicestrip";
pub const MAGIC: u64 = u64::from_be_bytes(*b"VSTRIP01");
pub const VERSION: u32 = 2;

/// Frames per slot: the granularity of validation.
pub const SLOT_FRAMES: usize = 32;
/// Slots in the ring; history is `SLOTS * SLOT_FRAMES` frames (341 ms).
pub const SLOTS: usize = 512;
pub const RING_FRAMES: usize = SLOTS * SLOT_FRAMES;

/// Segment header, at offset 0.
#[repr(C)]
pub struct Header {
    pub magic: AtomicU64,
    pub version: AtomicU32,
    /// Total segment size, bytes.
    pub size: AtomicU32,
    /// Random, non-zero, new at every app start.
    pub generation: AtomicU64,
    pub writer_pid: AtomicU32,
    pub slot_frames: AtomicU32,
    pub slots: AtomicU32,
    _pad: AtomicU32,
    /// Incremented by the app about every 100 ms while it runs.
    pub heartbeat: AtomicU64,
}

/// One slot's bookkeeping.
#[repr(C)]
pub struct Slot {
    /// Odd while the writer changes the slot.
    seq: AtomicU64,
    /// Absolute position of the slot's first frame; -1 when never written.
    start: AtomicI64,
    /// Valid frames are `[begin, end)` within the slot.
    begin: AtomicU32,
    end: AtomicU32,
}

/// The whole segment.
#[repr(C)]
pub struct Segment {
    pub header: Header,
    slots: [Slot; SLOTS],
    /// f32 bits.
    samples: [AtomicU32; RING_FRAMES],
}

pub const SIZE: usize = size_of::<Segment>();

/// A consistent copy of the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderView {
    pub generation: u64,
    pub writer_pid: u32,
    pub heartbeat: u64,
}

impl Header {
    /// Reads and validates the header; `None` if it is not a segment of this
    /// version and geometry (not yet initialized, or another layout).
    pub fn view(&self) -> Option<HeaderView> {
        let magic = self.magic.load(Relaxed);
        fence(Acquire);
        if magic != MAGIC
            || self.version.load(Relaxed) != VERSION
            || self.size.load(Relaxed) as usize != SIZE
            || self.slot_frames.load(Relaxed) as usize != SLOT_FRAMES
            || self.slots.load(Relaxed) as usize != SLOTS
        {
            return None;
        }
        let v = HeaderView {
            generation: self.generation.load(Relaxed),
            writer_pid: self.writer_pid.load(Relaxed),
            heartbeat: self.heartbeat.load(Relaxed),
        };
        fence(Acquire);
        (v.generation != 0).then_some(v)
    }
}

/// The slot holding absolute position `pos`: (slot start, offset in slot,
/// slot index).
fn locate(pos: i64) -> (i64, usize, usize) {
    let start = pos.div_euclid(SLOT_FRAMES as i64) * SLOT_FRAMES as i64;
    let index = (start / SLOT_FRAMES as i64).rem_euclid(SLOTS as i64) as usize;
    (start, (pos - start) as usize, index)
}

impl Segment {
    /// Initializes a fresh segment. The magic is stored last, with release
    /// ordering, so a reader that sees it sees every other field.
    pub fn init(&self, generation: u64, writer_pid: u32) {
        for s in &self.slots {
            s.seq.store(0, Relaxed);
            s.start.store(-1, Relaxed);
            s.begin.store(0, Relaxed);
            s.end.store(0, Relaxed);
        }
        let h = &self.header;
        h.version.store(VERSION, Relaxed);
        h.size.store(SIZE as u32, Relaxed);
        h.generation.store(generation, Relaxed);
        h.writer_pid.store(writer_pid, Relaxed);
        h.slot_frames.store(SLOT_FRAMES as u32, Relaxed);
        h.slots.store(SLOTS as u32, Relaxed);
        h.heartbeat.store(0, Relaxed);
        h.magic.store(MAGIC, Release);
    }

    /// Writes `samples` at absolute positions `pos..`. Only one thread may
    /// write. Realtime-safe: no allocation, no locks, no waiting.
    pub fn write(&self, pos: i64, samples: &[f32]) {
        let mut pos = pos;
        let mut rest = samples;
        while !rest.is_empty() {
            let (start, offset, index) = locate(pos);
            let n = rest.len().min(SLOT_FRAMES - offset);
            let slot = &self.slots[index];

            let seq = slot.seq.load(Relaxed);
            slot.seq.store(seq + 1, Relaxed);
            fence(Release);
            // A new position for the slot, or a gap in it, restarts its
            // valid range at this write.
            let contiguous =
                slot.start.load(Relaxed) == start && slot.end.load(Relaxed) as usize == offset;
            let begin = if contiguous {
                slot.begin.load(Relaxed)
            } else {
                offset as u32
            };
            slot.start.store(start, Relaxed);
            slot.begin.store(begin, Relaxed);
            let base = index * SLOT_FRAMES + offset;
            for (cell, s) in self.samples[base..base + n].iter().zip(&rest[..n]) {
                cell.store(s.to_bits(), Relaxed);
            }
            slot.end.store((offset + n) as u32, Relaxed);
            slot.seq.store(seq + 2, Release);

            pos += n as i64;
            rest = &rest[n..];
        }
    }

    /// Reads absolute positions `pos..pos + out.len()` into `out`. Frames
    /// not in the ring, or being rewritten, are zeroed. Returns how many
    /// frames were valid. Realtime-safe; never waits for the writer.
    pub fn read(&self, pos: i64, out: &mut [f32]) -> usize {
        let mut valid = 0;
        let mut pos = pos;
        let mut done = 0;
        while done < out.len() {
            let (start, offset, index) = locate(pos);
            let n = (out.len() - done).min(SLOT_FRAMES - offset);
            let slot = &self.slots[index];
            let dst = &mut out[done..done + n];

            // The part of this chunk the slot holds: [lo, hi) within the slot.
            let s1 = slot.seq.load(Relaxed);
            fence(Acquire);
            let (lo, hi) = if s1.is_multiple_of(2) && slot.start.load(Relaxed) == start {
                // Clamped to the chunk: values read during a racing write
                // are discarded below, but must not index out of it.
                let lo = (slot.begin.load(Relaxed) as usize).clamp(offset, offset + n);
                let hi = (slot.end.load(Relaxed) as usize).clamp(lo, offset + n);
                (lo, hi)
            } else {
                (offset, offset)
            };
            let base = index * SLOT_FRAMES;
            for (d, cell) in dst[lo - offset..hi - offset]
                .iter_mut()
                .zip(&self.samples[base + lo..base + hi])
            {
                *d = f32::from_bits(cell.load(Relaxed));
            }
            fence(Acquire);
            if slot.seq.load(Relaxed) == s1 {
                dst[..lo - offset].fill(0.0);
                dst[hi - offset..].fill(0.0);
                valid += hi - lo;
            } else {
                dst.fill(0.0);
            }

            pos += n as i64;
            done += n;
        }
        valid
    }
}

#[cfg(unix)]
pub use mapping::{Mapping, create, open};

#[cfg(unix)]
mod mapping {
    use std::ffi::CStr;
    use std::io;

    use super::{NAME, SIZE, Segment};

    /// A mapped segment. Unmapped on drop.
    pub struct Mapping {
        ptr: *mut libc::c_void,
    }

    // SAFETY: the mapping is plain shared memory accessed through atomics.
    unsafe impl Send for Mapping {}
    // SAFETY: as above.
    unsafe impl Sync for Mapping {}

    impl Mapping {
        pub fn segment(&self) -> &Segment {
            // SAFETY: the mapping is SIZE bytes, page aligned, and Segment is
            // made of atomics, valid for any bit pattern.
            unsafe { &*(self.ptr as *const Segment) }
        }

        /// Removes the segment name, for the writer on shutdown. Existing
        /// mappings stay valid.
        pub fn unlink(&self) {
            // SAFETY: valid C string.
            unsafe { libc::shm_unlink(NAME.as_ptr()) };
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: ptr comes from a successful mmap of SIZE bytes.
            unsafe { libc::munmap(self.ptr, SIZE) };
        }
    }

    /// shm_open is variadic on macOS and takes a fixed mode on Linux.
    ///
    /// # Safety
    ///
    /// `name` is a valid C string.
    unsafe fn shm_open(name: &CStr, oflag: libc::c_int, mode: libc::mode_t) -> libc::c_int {
        #[cfg(target_os = "macos")]
        // SAFETY: forwarded; the mode is the variadic argument.
        return unsafe { libc::shm_open(name.as_ptr(), oflag, libc::c_uint::from(mode)) };
        #[cfg(not(target_os = "macos"))]
        // SAFETY: forwarded.
        return unsafe { libc::shm_open(name.as_ptr(), oflag, mode) };
    }

    fn map(fd: libc::c_int, prot: libc::c_int) -> io::Result<Mapping> {
        // SAFETY: valid fd; the result is checked.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), SIZE, prot, libc::MAP_SHARED, fd, 0) };
        // SAFETY: the mapping holds its own reference to the object.
        unsafe { libc::close(fd) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { ptr })
    }

    /// Creates a new segment, replacing any previous one, and initializes
    /// it. Readers still mapping the old one keep it until they remap; the
    /// new generation tells them to.
    pub fn create(generation: u64) -> io::Result<Mapping> {
        // SAFETY: valid C string. A missing segment is fine.
        unsafe { libc::shm_unlink(NAME.as_ptr()) };
        // SAFETY: valid C string.
        let fd = unsafe { shm_open(NAME, libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o644) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: valid fd. The mode is the one given to shm_open minus the
        // umask: macOS rejects fchmod on shm objects.
        if unsafe { libc::ftruncate(fd, SIZE as libc::off_t) } != 0 {
            let e = io::Error::last_os_error();
            // SAFETY: valid fd and name.
            unsafe {
                libc::close(fd);
                libc::shm_unlink(NAME.as_ptr());
            }
            return Err(e);
        }
        let m = map(fd, libc::PROT_READ | libc::PROT_WRITE)?;
        m.segment().init(generation, std::process::id());
        Ok(m)
    }

    /// Maps the current segment read-only.
    pub fn open() -> io::Result<Mapping> {
        // SAFETY: valid C string.
        let fd = unsafe { shm_open(NAME, libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: zeroed stat is a valid out buffer for fstat.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and out buffer.
        if unsafe { libc::fstat(fd, &mut st) } != 0 || (st.st_size as usize) < SIZE {
            let e = io::Error::new(io::ErrorKind::InvalidData, "segment too small");
            // SAFETY: valid fd.
            unsafe { libc::close(fd) };
            return Err(e);
        }
        map(fd, libc::PROT_READ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A segment in ordinary memory, for protocol tests.
    fn segment() -> Box<Segment> {
        // SAFETY: Segment is made of atomics; all-zero is a valid value.
        let s: Box<Segment> = unsafe { Box::new_zeroed().assume_init() };
        s.init(1, 1);
        s
    }

    fn ramp(pos: i64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (pos + i as i64) as f32).collect()
    }

    #[test]
    fn layout_is_fixed() {
        assert_eq!(size_of::<Header>(), 48);
        assert_eq!(size_of::<Slot>(), 24);
        assert_eq!(SIZE, 48 + SLOTS * 24 + RING_FRAMES * 4);
        assert!(NAME.count_bytes() <= 31);
    }

    #[test]
    fn a_header_is_valid_only_after_init() {
        // SAFETY: as in `segment`.
        let s: Box<Segment> = unsafe { Box::new_zeroed().assume_init() };
        assert_eq!(s.header.view(), None);
        s.init(7, 42);
        assert_eq!(
            s.header.view(),
            Some(HeaderView {
                generation: 7,
                writer_pid: 42,
                heartbeat: 0
            })
        );
    }

    #[test]
    fn written_frames_read_back_across_slots_and_wrap() {
        let s = segment();
        let pos = RING_FRAMES as i64 * 3 - 50;
        // Odd-sized writes that straddle slots and the ring end.
        let mut p = pos;
        for n in [7, 33, 64, 1, 95] {
            s.write(p, &ramp(p, n));
            p += n as i64;
        }
        let mut out = vec![0.0; 200];
        assert_eq!(s.read(pos, &mut out), 200);
        assert_eq!(out, ramp(pos, 200));
    }

    #[test]
    fn missing_old_and_partial_frames_read_as_silence() {
        let s = segment();
        s.write(1000, &ramp(1000, 64));
        let mut out = vec![9.0; 96];
        // 64 written, 32 never written.
        assert_eq!(s.read(1000, &mut out), 64);
        assert_eq!(&out[64..], &[0.0; 32]);
        // The same slots one ring later hold other positions.
        let later = 1000 + RING_FRAMES as i64;
        assert_eq!(s.read(later, &mut out[..64]), 0);
        assert!(out[..64].iter().all(|v| *v == 0.0));
        // A gap inside a slot invalidates what came before it.
        s.write(2048, &ramp(2048, 4));
        s.write(2056, &ramp(2056, 4));
        let mut four = [0.0; 4];
        assert_eq!(s.read(2048, &mut four), 0);
        assert_eq!(s.read(2056, &mut four), 4);
    }

    #[test]
    fn a_slot_being_written_reads_as_silence() {
        let s = segment();
        s.write(64, &ramp(64, 32));
        let slot = &s.slots[2];
        slot.seq.fetch_add(1, Relaxed);
        let mut out = [1.0; 32];
        assert_eq!(s.read(64, &mut out), 0);
        slot.seq.fetch_add(1, Relaxed);
        assert_eq!(s.read(64, &mut out), 32);
    }

    #[test]
    fn a_concurrent_reader_never_sees_a_torn_frame() {
        let s = segment();
        let head = AtomicI64::new(0);
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|sc| {
            sc.spawn(|| {
                let mut p = 0i64;
                while !stop.load(Relaxed) {
                    s.write(p, &ramp(p, 48));
                    p += 48;
                    head.store(p, Relaxed);
                }
            });
            let mut out = vec![0.0; 64];
            let mut checked = 0usize;
            for i in 0..200_000i64 {
                // Just behind the writer, where slots are being rewritten.
                let pos = (head.load(Relaxed) - 64 - i % 4000).max(0);
                checked += s.read(pos, &mut out);
                for (k, v) in out.iter().enumerate() {
                    // Every frame is either silence or exactly its position.
                    assert!(
                        *v == 0.0 || *v == (pos + k as i64) as f32,
                        "torn at {pos}+{k}"
                    );
                }
            }
            stop.store(true, Relaxed);
            assert!(checked > 0);
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn create_then_open_sees_the_same_segment() {
        let w = create(99).unwrap();
        let r = open().unwrap();
        assert_eq!(r.segment().header.view().unwrap().generation, 99);
        w.segment().write(0, &[0.5; 8]);
        let mut out = [0.0; 8];
        assert_eq!(r.segment().read(0, &mut out), 8);
        w.unlink();
        assert!(open().is_err());
    }
}
