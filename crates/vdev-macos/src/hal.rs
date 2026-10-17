//! Client side of the Core Audio HAL: find a device by UID and capture its
//! input with an IOProc.

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering::*};
use std::time::Duration;

use vdev_macos_driver::hal::{
    AudioObjectId, ELEMENT_MAIN, NO_ERROR, OsStatus, PROP_DEVICE_LIST, PROP_DEVICE_UID,
    PROP_LATENCY, PROP_NAME, PROP_NOMINAL_SAMPLE_RATE, PROP_SAFETY_OFFSET,
    PROP_STREAM_VIRTUAL_FORMAT, PROP_STREAMS, PropertyAddress, SCOPE_GLOBAL, SCOPE_INPUT,
    SCOPE_OUTPUT, StreamBasicDescription, TimeStamp, fourcc,
};

use crate::probe::Block;

const SYSTEM_OBJECT: AudioObjectId = 1;
/// kAudioHardwarePropertyDevices, the same code as the plug-in's device list.
const PROP_DEVICES: u32 = PROP_DEVICE_LIST;
const PROP_BUFFER_FRAME_SIZE: u32 = fourcc(b"fsiz");
const PROP_DEFAULT_INPUT_DEVICE: u32 = fourcc(b"dIn ");
const PROP_DEFAULT_OUTPUT_DEVICE: u32 = fourcc(b"dOut");
const UTF8: u32 = 0x0800_0100;

#[repr(C)]
pub struct AudioBuffer {
    pub channels: u32,
    pub bytes: u32,
    pub data: *mut c_void,
}

/// AudioBufferList; the HAL passes `count` buffers, of which only the first
/// is declared here.
#[repr(C)]
pub struct AudioBufferList {
    pub count: u32,
    pub buffers: [AudioBuffer; 1],
}

pub type IoProc = unsafe extern "C" fn(
    AudioObjectId,
    *const TimeStamp,
    *const AudioBufferList,
    *const TimeStamp,
    *mut AudioBufferList,
    *const TimeStamp,
    *mut c_void,
) -> OsStatus;
pub type IoProcId = *mut c_void;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: AudioObjectId,
        addr: *const PropertyAddress,
        q_size: u32,
        q: *const c_void,
        out: *mut u32,
    ) -> OsStatus;
    fn AudioObjectGetPropertyData(
        id: AudioObjectId,
        addr: *const PropertyAddress,
        q_size: u32,
        q: *const c_void,
        io_size: *mut u32,
        out: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        id: AudioObjectId,
        addr: *const PropertyAddress,
        q_size: u32,
        q: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
    fn AudioDeviceCreateIOProcID(
        dev: AudioObjectId,
        proc_: IoProc,
        client: *mut c_void,
        out: *mut IoProcId,
    ) -> OsStatus;
    fn AudioDeviceDestroyIOProcID(dev: AudioObjectId, id: IoProcId) -> OsStatus;
    fn AudioDeviceStart(dev: AudioObjectId, id: IoProcId) -> OsStatus;
    fn AudioDeviceStop(dev: AudioObjectId, id: IoProcId) -> OsStatus;
}

#[repr(C)]
struct TimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(cf: *const c_void);
}

fn global(selector: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope: SCOPE_GLOBAL,
        element: ELEMENT_MAIN,
    }
}

fn check(status: OsStatus) -> Result<(), OsStatus> {
    if status == NO_ERROR {
        Ok(())
    } else {
        Err(status)
    }
}

/// All devices the HAL knows.
pub fn devices() -> Result<Vec<AudioObjectId>, OsStatus> {
    let addr = global(PROP_DEVICES);
    let mut size = 0u32;
    // SAFETY: valid address and out pointer.
    check(unsafe {
        AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut size)
    })?;
    let mut ids = vec![0 as AudioObjectId; size as usize / 4];
    // SAFETY: the buffer holds `size` bytes.
    check(unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            ids.as_mut_ptr() as *mut c_void,
        )
    })?;
    ids.truncate(size as usize / 4);
    Ok(ids)
}

/// A CFString property of an object, as a Rust string.
pub fn string(id: AudioObjectId, selector: u32) -> Result<String, OsStatus> {
    let addr = global(selector);
    let mut cf: *const c_void = std::ptr::null();
    let mut size = size_of::<*const c_void>() as u32;
    // SAFETY: the out buffer holds one CFStringRef.
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut cf as *mut _ as *mut c_void,
        )
    })?;
    if cf.is_null() {
        return Ok(String::new());
    }
    let mut buf = [0u8; 512];
    // SAFETY: valid CFString (owned, released below) and buffer.
    let ok = unsafe {
        CFStringGetCString(
            cf,
            buf.as_mut_ptr() as *mut c_char,
            buf.len() as isize,
            UTF8,
        )
    };
    // SAFETY: the HAL returned a +1 reference.
    unsafe { CFRelease(cf) };
    let len = buf.iter().position(|b| *b == 0).unwrap_or(0);
    Ok(if ok != 0 {
        String::from_utf8_lossy(&buf[..len]).into_owned()
    } else {
        String::new()
    })
}

pub fn name(id: AudioObjectId) -> Result<String, OsStatus> {
    string(id, PROP_NAME)
}

/// The device with this UID, if the HAL has it.
pub fn find(uid: &str) -> Result<Option<AudioObjectId>, OsStatus> {
    for id in devices()? {
        if string(id, PROP_DEVICE_UID).is_ok_and(|u| u == uid) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn scoped(selector: u32, scope: u32) -> PropertyAddress {
    PropertyAddress {
        selector,
        scope,
        element: ELEMENT_MAIN,
    }
}

/// Reads a plain-data property into `T`.
fn read<T: Copy>(id: AudioObjectId, addr: PropertyAddress, init: T) -> Result<T, OsStatus> {
    let mut v = init;
    let mut size = size_of::<T>() as u32;
    // SAFETY: the out buffer holds one T, a plain-data HAL type.
    check(unsafe {
        AudioObjectGetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut v as *mut T as *mut c_void,
        )
    })?;
    Ok(v)
}

/// Whether the device has streams in `scope` (input or output).
fn has_streams(id: AudioObjectId, scope: u32) -> bool {
    let addr = scoped(PROP_STREAMS, scope);
    let mut size = 0u32;
    // SAFETY: valid address and out pointer.
    let ok = unsafe { AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) };
    ok == NO_ERROR && size > 0
}

pub fn has_input(id: AudioObjectId) -> bool {
    has_streams(id, SCOPE_INPUT)
}

pub fn has_output(id: AudioObjectId) -> bool {
    has_streams(id, SCOPE_OUTPUT)
}

pub fn uid(id: AudioObjectId) -> Result<String, OsStatus> {
    string(id, PROP_DEVICE_UID)
}

/// The system default input device.
pub fn default_input() -> Result<AudioObjectId, OsStatus> {
    read(SYSTEM_OBJECT, global(PROP_DEFAULT_INPUT_DEVICE), 0u32)
}

/// The system default output device.
pub fn default_output() -> Result<AudioObjectId, OsStatus> {
    read(SYSTEM_OBJECT, global(PROP_DEFAULT_OUTPUT_DEVICE), 0u32)
}

pub fn nominal_rate(id: AudioObjectId) -> Result<f64, OsStatus> {
    read(id, global(PROP_NOMINAL_SAMPLE_RATE), 0f64)
}

/// The virtual format of the device's first input stream: the format its
/// IOProc receives.
pub fn input_format(id: AudioObjectId) -> Result<StreamBasicDescription, OsStatus> {
    stream_format(id, SCOPE_INPUT)
}

/// The virtual format of the device's first output stream: the format its
/// IOProc writes.
pub fn output_format(id: AudioObjectId) -> Result<StreamBasicDescription, OsStatus> {
    stream_format(id, SCOPE_OUTPUT)
}

fn stream_format(id: AudioObjectId, scope: u32) -> Result<StreamBasicDescription, OsStatus> {
    let stream = read(id, scoped(PROP_STREAMS, scope), 0u32)?;
    read(
        stream,
        global(PROP_STREAM_VIRTUAL_FORMAT),
        StreamBasicDescription {
            sample_rate: 0.0,
            format_id: 0,
            format_flags: 0,
            bytes_per_packet: 0,
            frames_per_packet: 0,
            bytes_per_frame: 0,
            channels_per_frame: 0,
            bits_per_channel: 0,
            reserved: 0,
        },
    )
}

/// Input latency the device reports beyond its buffers: device latency
/// plus safety offset, frames.
pub fn input_latency(id: AudioObjectId) -> u32 {
    device_latency(id, SCOPE_INPUT)
}

/// Output latency the device reports beyond its buffers, frames.
pub fn output_latency(id: AudioObjectId) -> u32 {
    device_latency(id, SCOPE_OUTPUT)
}

fn device_latency(id: AudioObjectId, scope: u32) -> u32 {
    let l = read(id, scoped(PROP_LATENCY, scope), 0u32).unwrap_or(0);
    let s = read(id, scoped(PROP_SAFETY_OFFSET, scope), 0u32).unwrap_or(0);
    l + s
}

/// Asks for an IO buffer of `frames` for this process; returns what the
/// device grants.
pub fn set_buffer_frame_size(id: AudioObjectId, frames: u32) -> Result<u32, OsStatus> {
    let addr = global(PROP_BUFFER_FRAME_SIZE);
    // SAFETY: the data is one UInt32.
    check(unsafe {
        AudioObjectSetPropertyData(
            id,
            &addr,
            0,
            std::ptr::null(),
            4,
            &frames as *const u32 as *const c_void,
        )
    })?;
    buffer_frame_size(id)
}

/// Creates and starts an IOProc on `dev`.
///
/// # Safety
///
/// `client` stays valid until [`stop_io_proc`] returns.
pub unsafe fn start_io_proc(
    dev: AudioObjectId,
    proc_: IoProc,
    client: *mut c_void,
) -> Result<IoProcId, OsStatus> {
    let mut id: IoProcId = std::ptr::null_mut();
    // SAFETY: forwarded from the caller.
    check(unsafe { AudioDeviceCreateIOProcID(dev, proc_, client, &mut id) })?;
    // SAFETY: valid device and IOProc.
    if let Err(e) = check(unsafe { AudioDeviceStart(dev, id) }) {
        // SAFETY: as above.
        unsafe { AudioDeviceDestroyIOProcID(dev, id) };
        return Err(e);
    }
    Ok(id)
}

/// Stops and destroys an IOProc; it is not called again after this returns.
///
/// # Safety
///
/// `id` comes from [`start_io_proc`] on `dev`.
pub unsafe fn stop_io_proc(dev: AudioObjectId, id: IoProcId) {
    // SAFETY: forwarded from the caller.
    unsafe {
        AudioDeviceStop(dev, id);
        AudioDeviceDestroyIOProcID(dev, id);
    }
}

/// The device's IO buffer size, frames.
pub fn buffer_frame_size(id: AudioObjectId) -> Result<u32, OsStatus> {
    read(id, global(PROP_BUFFER_FRAME_SIZE), 0u32)
}

/// The device's reported input latency, frames.
pub fn latency(id: AudioObjectId) -> Result<u32, OsStatus> {
    read(id, scoped(PROP_LATENCY, SCOPE_INPUT), 0u32)
}

/// Host time now, in host ticks (mach_absolute_time).
pub fn host_now() -> u64 {
    // SAFETY: plain system call.
    unsafe { mach_absolute_time() }
}

/// Host ticks per second.
pub fn host_ticks_per_second() -> f64 {
    let mut tb = TimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: valid out pointer.
    unsafe { mach_timebase_info(&mut tb) };
    1e9 * f64::from(tb.denom) / f64::from(tb.numer.max(1))
}

/// Preallocated capture storage the IOProc fills without allocating.
struct Recorder {
    samples: Vec<f32>,
    blocks: Vec<Block>,
    n_samples: AtomicUsize,
    n_blocks: AtomicUsize,
}

unsafe extern "C" fn record(
    _dev: AudioObjectId,
    _now: *const TimeStamp,
    input: *const AudioBufferList,
    input_time: *const TimeStamp,
    _output: *mut AudioBufferList,
    _output_time: *const TimeStamp,
    client: *mut c_void,
) -> OsStatus {
    // SAFETY: `client` is the Recorder passed to AudioDeviceCreateIOProcID,
    // alive until the IOProc is destroyed; only this thread writes to it.
    let rec = unsafe { &mut *(client as *mut Recorder) };
    // SAFETY: the HAL passes valid buffers and time stamps.
    let (Some(list), Some(time)) = (unsafe { input.as_ref() }, unsafe { input_time.as_ref() })
    else {
        return NO_ERROR;
    };
    if list.count == 0 || list.buffers[0].data.is_null() {
        return NO_ERROR;
    }
    let b = &list.buffers[0];
    let frames = (b.bytes / 4 / b.channels.max(1)) as usize;
    // SAFETY: the buffer holds `bytes` bytes of interleaved f32.
    let data = unsafe {
        std::slice::from_raw_parts(b.data as *const f32, frames * b.channels.max(1) as usize)
    };
    let ns = rec.n_samples.load(Relaxed);
    let nb = rec.n_blocks.load(Relaxed);
    if ns + frames > rec.samples.len() || nb == rec.blocks.len() {
        return NO_ERROR;
    }
    let step = b.channels.max(1) as usize;
    for (dst, src) in rec.samples[ns..ns + frames]
        .iter_mut()
        .zip(data.iter().step_by(step))
    {
        *dst = *src;
    }
    rec.blocks[nb] = Block {
        sample_time: time.sample_time,
        frames: frames as u32,
    };
    rec.n_samples.store(ns + frames, Release);
    rec.n_blocks.store(nb + 1, Release);
    NO_ERROR
}

/// Captures `duration` of the device's first input channel.
pub fn capture(
    dev: AudioObjectId,
    duration: Duration,
    sample_rate: f64,
) -> Result<(Vec<f32>, Vec<Block>), OsStatus> {
    let frames = (duration.as_secs_f64() * sample_rate) as usize;
    // Headroom for the IO cycle that crosses `frames`.
    let mut rec = Box::new(Recorder {
        samples: vec![0.0; frames + 16_384],
        blocks: vec![
            Block {
                sample_time: 0.0,
                frames: 0
            };
            frames / 16 + 16
        ],
        n_samples: AtomicUsize::new(0),
        n_blocks: AtomicUsize::new(0),
    });
    let client = &mut *rec as *mut Recorder as *mut c_void;
    // SAFETY: `rec` outlives the IOProc, stopped below.
    let id = unsafe { start_io_proc(dev, record, client) }?;
    let deadline = std::time::Instant::now() + 3 * duration;
    while rec.n_samples.load(Acquire) < frames && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: from start_io_proc on `dev`.
    unsafe { stop_io_proc(dev, id) };
    let ns = rec.n_samples.load(Acquire);
    let nb = rec.n_blocks.load(Acquire);
    let Recorder {
        mut samples,
        mut blocks,
        ..
    } = *rec;
    samples.truncate(ns.min(frames));
    blocks.truncate(nb);
    Ok((samples, blocks))
}
