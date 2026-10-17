//! The C side of the plug-in: the CFPlugIn factory, the
//! AudioServerPlugInDriverInterface table `coreaudiod` calls, and the
//! conversions between its raw pointers and [`crate::model`].
//!
//! The driver is one static object: a pointer to the interface table, as the
//! COM-style CFPlugIn convention wants (`AudioServerPlugInDriverRef` is a
//! pointer to a pointer to the table). Its state is a handful of atomics.
//! The IO entry points (`GetZeroTimeStamp`, `DoIOOperation`, ...) run on the
//! HAL's realtime IO thread: they only touch atomics and the IO buffer.

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering::*};

use crate::clock;
use crate::hal::*;
use crate::model::{self, Change, DEVICE, Qualifier, STREAM, State, Value};
use crate::shm;

type CfAllocatorRef = *const c_void;
type CfUuidRef = *const c_void;
type CfStringRef = *const c_void;
type CfDictionaryRef = *const c_void;
type DriverRef = *mut c_void;
type HostRef = *const c_void;
type Pid = i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct CfUuidBytes([u8; 16]);

const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFUUIDGetUUIDBytes(uuid: CfUuidRef) -> CfUuidBytes;
    fn CFStringCreateWithBytes(
        alloc: CfAllocatorRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        is_external: u8,
    ) -> CfStringRef;
    fn CFStringGetCString(s: CfStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
}

#[repr(C)]
struct TimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut TimebaseInfo) -> c_int;
    fn syslog(priority: c_int, format: *const c_char, ...);
}

/// kAudioServerPlugInTypeUUID, 443ABAB8-E7B3-491A-B985-BEB9187030DB.
const PLUGIN_TYPE: CfUuidBytes = CfUuidBytes([
    0x44, 0x3A, 0xBA, 0xB8, 0xE7, 0xB3, 0x49, 0x1A, 0xB9, 0x85, 0xBE, 0xB9, 0x18, 0x70, 0x30, 0xDB,
]);
/// kAudioServerPlugInDriverInterfaceUUID, EEA5773D-CC43-49F1-8E00-8F96E7D23B17.
const DRIVER_INTERFACE: CfUuidBytes = CfUuidBytes([
    0xEE, 0xA5, 0x77, 0x3D, 0xCC, 0x43, 0x49, 0xF1, 0x8E, 0x00, 0x8F, 0x96, 0xE7, 0xD2, 0x3B, 0x17,
]);
/// IUnknownUUID, 00000000-0000-0000-C000-000000000046.
const IUNKNOWN: CfUuidBytes = CfUuidBytes([0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x8000_0004_u32 as i32;
const LOG_NOTICE: c_int = 5;

fn log(msg: &str) {
    if let Ok(c) = CString::new(format!("VoiceStrip: {msg}")) {
        // SAFETY: a "%s" format with one valid C string argument.
        unsafe { syslog(LOG_NOTICE, c"%s".as_ptr(), c.as_ptr()) };
    }
}

fn fourcc_str(v: u32) -> String {
    v.to_be_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                *b as char
            } else {
                '?'
            }
        })
        .collect()
}

fn describe(object: u32, a: &PropertyAddress) -> String {
    format!(
        "object {object} '{}' scope '{}' element {}",
        fourcc_str(a.selector),
        fourcc_str(a.scope),
        a.element
    )
}

// ---------------------------------------------------------------- state

static REFS: AtomicU32 = AtomicU32::new(1);
static IO_CLIENTS: AtomicU32 = AtomicU32::new(0);
/// [`clock::ticks_per_frame`] as f64 bits, set in Initialize.
static TICKS_PER_FRAME: AtomicU64 = AtomicU64::new(0);
static STREAM_ACTIVE: AtomicBool = AtomicBool::new(true);

/// The app's segment, owned by the watcher thread; null while none is
/// attached. The IO thread uses it between incrementing and decrementing
/// [`READERS`]; the watcher frees a replaced mapping only once it has seen
/// [`READERS`] at zero after the swap.
static CURRENT: AtomicPtr<shm::Mapping> = AtomicPtr::new(std::ptr::null_mut());
static READERS: AtomicU32 = AtomicU32::new(0);
/// Frames delivered as silence because the ring did not hold them.
static MISSING: AtomicU64 = AtomicU64::new(0);

fn state() -> State {
    State {
        running: IO_CLIENTS.load(Relaxed) > 0,
        stream_active: STREAM_ACTIVE.load(Relaxed),
    }
}

// ---------------------------------------------------------------- interface

#[repr(C)]
struct DriverInterface {
    _reserved: usize,
    query_interface: unsafe extern "C" fn(*mut c_void, CfUuidBytes, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
    release: unsafe extern "C" fn(*mut c_void) -> u32,
    initialize: unsafe extern "C" fn(DriverRef, HostRef) -> OsStatus,
    create_device: unsafe extern "C" fn(
        DriverRef,
        CfDictionaryRef,
        *const c_void,
        *mut AudioObjectId,
    ) -> OsStatus,
    destroy_device: unsafe extern "C" fn(DriverRef, AudioObjectId) -> OsStatus,
    add_device_client: unsafe extern "C" fn(DriverRef, AudioObjectId, *const c_void) -> OsStatus,
    remove_device_client: unsafe extern "C" fn(DriverRef, AudioObjectId, *const c_void) -> OsStatus,
    perform_device_configuration_change:
        unsafe extern "C" fn(DriverRef, AudioObjectId, u64, *mut c_void) -> OsStatus,
    abort_device_configuration_change:
        unsafe extern "C" fn(DriverRef, AudioObjectId, u64, *mut c_void) -> OsStatus,
    has_property: unsafe extern "C" fn(DriverRef, AudioObjectId, Pid, *const PropertyAddress) -> u8,
    is_property_settable: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        Pid,
        *const PropertyAddress,
        *mut u8,
    ) -> OsStatus,
    get_property_data_size: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        Pid,
        *const PropertyAddress,
        u32,
        *const c_void,
        *mut u32,
    ) -> OsStatus,
    get_property_data: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        Pid,
        *const PropertyAddress,
        u32,
        *const c_void,
        u32,
        *mut u32,
        *mut c_void,
    ) -> OsStatus,
    set_property_data: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        Pid,
        *const PropertyAddress,
        u32,
        *const c_void,
        u32,
        *const c_void,
    ) -> OsStatus,
    start_io: unsafe extern "C" fn(DriverRef, AudioObjectId, u32) -> OsStatus,
    stop_io: unsafe extern "C" fn(DriverRef, AudioObjectId, u32) -> OsStatus,
    get_zero_time_stamp: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        u32,
        *mut f64,
        *mut u64,
        *mut u64,
    ) -> OsStatus,
    will_do_io_operation:
        unsafe extern "C" fn(DriverRef, AudioObjectId, u32, u32, *mut u8, *mut u8) -> OsStatus,
    begin_io_operation: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        u32,
        u32,
        u32,
        *const IoCycleInfo,
    ) -> OsStatus,
    do_io_operation: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        AudioObjectId,
        u32,
        u32,
        u32,
        *const IoCycleInfo,
        *mut c_void,
        *mut c_void,
    ) -> OsStatus,
    end_io_operation: unsafe extern "C" fn(
        DriverRef,
        AudioObjectId,
        u32,
        u32,
        u32,
        *const IoCycleInfo,
    ) -> OsStatus,
}

static INTERFACE: DriverInterface = DriverInterface {
    _reserved: 0,
    query_interface,
    add_ref,
    release,
    initialize,
    create_device,
    destroy_device,
    add_device_client,
    remove_device_client,
    perform_device_configuration_change,
    abort_device_configuration_change,
    has_property,
    is_property_settable,
    get_property_data_size,
    get_property_data,
    set_property_data,
    start_io,
    stop_io,
    get_zero_time_stamp,
    will_do_io_operation,
    begin_io_operation,
    do_io_operation,
    end_io_operation,
};

/// The driver object: a pointer to the interface table.
#[repr(C)]
struct Driver {
    _interface: &'static DriverInterface,
}
static DRIVER: Driver = Driver {
    _interface: &INTERFACE,
};

fn driver_ref() -> *mut c_void {
    &DRIVER as *const Driver as *mut c_void
}

/// The CFPlugIn factory named in `Info.plist`.
///
/// # Safety
///
/// Called by CFPlugIn with a valid CFUUID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn VoiceStrip_Create(
    _alloc: CfAllocatorRef,
    type_uuid: CfUuidRef,
) -> *mut c_void {
    // SAFETY: CFPlugIn passes a valid CFUUIDRef.
    if type_uuid.is_null() || unsafe { CFUUIDGetUUIDBytes(type_uuid) } != PLUGIN_TYPE {
        return std::ptr::null_mut();
    }
    log("factory: driver created");
    driver_ref()
}

unsafe extern "C" fn query_interface(
    _d: *mut c_void,
    uuid: CfUuidBytes,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_NOINTERFACE;
    }
    if uuid == DRIVER_INTERFACE || uuid == IUNKNOWN {
        REFS.fetch_add(1, Relaxed);
        // SAFETY: `out` is a valid out pointer per the COM contract.
        unsafe { *out = driver_ref() };
        S_OK
    } else {
        // SAFETY: as above.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "C" fn add_ref(_d: *mut c_void) -> u32 {
    REFS.fetch_add(1, Relaxed) + 1
}

unsafe extern "C" fn release(_d: *mut c_void) -> u32 {
    // The driver is static: the count only mirrors the protocol.
    REFS.fetch_sub(1, Relaxed).saturating_sub(1)
}

unsafe extern "C" fn initialize(_d: DriverRef, _host: HostRef) -> OsStatus {
    let mut tb = TimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: valid out pointer.
    unsafe { mach_timebase_info(&mut tb) };
    let ticks_per_second = 1e9 * f64::from(tb.denom) / f64::from(tb.numer.max(1));
    TICKS_PER_FRAME.store(clock::ticks_per_frame(ticks_per_second).to_bits(), Relaxed);
    log(&format!(
        "initialize: {ticks_per_second:.0} host ticks/s, device '{}'",
        model::DEVICE_UID
    ));
    if let Err(e) = std::thread::Builder::new()
        .name("voicestrip-shm".into())
        .spawn(watch_segment)
    {
        log(&format!("shm: no watcher thread ({e})"));
    }
    NO_ERROR
}

/// Publishes `m` as the current segment and frees the one it replaces once
/// the IO thread no longer uses it.
fn publish(m: shm::Mapping) {
    let new = Box::into_raw(Box::new(m));
    let old = CURRENT.swap(new, SeqCst);
    if old.is_null() {
        return;
    }
    while READERS.load(SeqCst) != 0 {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    // SAFETY: `old` came from Box::into_raw, is no longer published, and no
    // reader holds it: any reader that loaded it was counted in READERS.
    drop(unsafe { Box::from_raw(old) });
}

/// Keeps the app's shared memory segment mapped, off the realtime threads:
/// maps it whenever a new generation appears (app started or restarted, in
/// any order with the HAL clients), logs when its writer stops beating and
/// reports frames that reached a client as silence.
fn watch_segment() {
    use std::time::{Duration, Instant};

    let mut generation: Option<u64> = None;
    let mut last_error: Option<String> = None;
    let mut beat = (0u64, Instant::now());
    let mut stale = false;
    let mut missing = (0u64, Instant::now());
    loop {
        match shm::open() {
            Ok(m) => match m.segment().header.view() {
                Some(v) if generation != Some(v.generation) => {
                    log(&format!(
                        "shm: attached generation {:016x} pid {}",
                        v.generation, v.writer_pid
                    ));
                    beat = (v.heartbeat, Instant::now());
                    stale = false;
                    last_error = None;
                    generation = Some(v.generation);
                    publish(m);
                }
                Some(_) => {}
                None => {
                    let e = "segment without a valid header".to_string();
                    if last_error.as_ref() != Some(&e) {
                        log(&format!("shm: {e}"));
                        last_error = Some(e);
                    }
                }
            },
            Err(e) => {
                let e = e.to_string();
                if last_error.as_ref() != Some(&e) {
                    log(&format!("shm: open failed: {e}"));
                    last_error = Some(e);
                }
            }
        }
        if let Some(g) = generation {
            // SAFETY: only this thread frees the published mapping.
            let hb = unsafe { CURRENT.load(SeqCst).as_ref() }
                .and_then(|m| m.segment().header.view())
                .map_or(beat.0, |v| v.heartbeat);
            if hb != beat.0 {
                beat = (hb, Instant::now());
                stale = false;
            } else if !stale && beat.1.elapsed() > Duration::from_secs(1) {
                log(&format!("shm: writer stale, generation {g:016x}"));
                stale = true;
            }
        }
        if missing.1.elapsed() >= Duration::from_secs(2) {
            let now = MISSING.load(Relaxed);
            if now != missing.0 && IO_CLIENTS.load(Relaxed) > 0 {
                log(&format!(
                    "shm: {} frames missing in the last 2 s",
                    now - missing.0
                ));
            }
            missing = (now, Instant::now());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

unsafe extern "C" fn create_device(
    _d: DriverRef,
    _desc: CfDictionaryRef,
    _client: *const c_void,
    _out: *mut AudioObjectId,
) -> OsStatus {
    ERR_UNSUPPORTED_OPERATION
}

unsafe extern "C" fn destroy_device(_d: DriverRef, _dev: AudioObjectId) -> OsStatus {
    ERR_UNSUPPORTED_OPERATION
}

unsafe extern "C" fn add_device_client(
    _d: DriverRef,
    _dev: AudioObjectId,
    _c: *const c_void,
) -> OsStatus {
    NO_ERROR
}

unsafe extern "C" fn remove_device_client(
    _d: DriverRef,
    _dev: AudioObjectId,
    _c: *const c_void,
) -> OsStatus {
    NO_ERROR
}

unsafe extern "C" fn perform_device_configuration_change(
    _d: DriverRef,
    _dev: AudioObjectId,
    _action: u64,
    _info: *mut c_void,
) -> OsStatus {
    NO_ERROR
}

unsafe extern "C" fn abort_device_configuration_change(
    _d: DriverRef,
    _dev: AudioObjectId,
    _action: u64,
    _info: *mut c_void,
) -> OsStatus {
    NO_ERROR
}

// ---------------------------------------------------------------- properties

/// Copies a CFString qualifier into `buf`, returning it as a str.
///
/// # Safety
///
/// `q` points to `size` bytes of qualifier data.
unsafe fn uid_qualifier(q: *const c_void, size: u32, buf: &mut [u8; 256]) -> Option<&str> {
    if q.is_null() || size as usize != size_of::<CfStringRef>() {
        return None;
    }
    // SAFETY: the qualifier holds one CFStringRef.
    let s = unsafe { *(q as *const CfStringRef) };
    if s.is_null() {
        return None;
    }
    // SAFETY: valid CFString and buffer of the declared size.
    let ok =
        unsafe { CFStringGetCString(s, buf.as_mut_ptr() as *mut c_char, buf.len() as isize, UTF8) };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|b| *b == 0)?;
    std::str::from_utf8(&buf[..len]).ok()
}

/// Decodes the qualifier for `selector` and runs `f` with it.
///
/// # Safety
///
/// `q` points to `size` bytes of qualifier data.
unsafe fn with_qualifier<R>(
    selector: u32,
    q: *const c_void,
    size: u32,
    f: impl FnOnce(&Qualifier) -> R,
) -> R {
    let mut buf = [0u8; 256];
    let mut qual = Qualifier::default();
    if selector == PROP_OWNED_OBJECTS && !q.is_null() && size >= 4 {
        // SAFETY: an array of AudioClassIDs of `size` bytes.
        qual.classes = unsafe { std::slice::from_raw_parts(q as *const u32, size as usize / 4) };
    }
    if selector == PROP_TRANSLATE_UID_TO_DEVICE {
        // SAFETY: forwarded from the caller.
        qual.uid = unsafe { uid_qualifier(q, size, &mut buf) };
    }
    f(&qual)
}

unsafe extern "C" fn has_property(
    _d: DriverRef,
    object: AudioObjectId,
    _pid: Pid,
    addr: *const PropertyAddress,
) -> u8 {
    // SAFETY: the HAL passes a valid address.
    let Some(a) = (unsafe { addr.as_ref() }) else {
        return 0;
    };
    let has = model::has(object, a);
    if !has {
        log(&format!("has: no {}", describe(object, a)));
    }
    has.into()
}

unsafe extern "C" fn is_property_settable(
    _d: DriverRef,
    object: AudioObjectId,
    _pid: Pid,
    addr: *const PropertyAddress,
    out: *mut u8,
) -> OsStatus {
    // SAFETY: the HAL passes a valid address.
    let (Some(a), false) = (unsafe { addr.as_ref() }, out.is_null()) else {
        return ERR_ILLEGAL_OPERATION;
    };
    match model::is_settable(object, a) {
        Ok(s) => {
            // SAFETY: checked non-null above.
            unsafe { *out = s.into() };
            NO_ERROR
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn get_property_data_size(
    _d: DriverRef,
    object: AudioObjectId,
    _pid: Pid,
    addr: *const PropertyAddress,
    q_size: u32,
    q: *const c_void,
    out: *mut u32,
) -> OsStatus {
    // SAFETY: the HAL passes a valid address.
    let (Some(a), false) = (unsafe { addr.as_ref() }, out.is_null()) else {
        return ERR_ILLEGAL_OPERATION;
    };
    let size = if object == OBJECT_PLUGIN && a.selector == PROP_TRANSLATE_UID_TO_DEVICE {
        Ok(4)
    } else {
        // SAFETY: qualifier pointer and size from the HAL.
        unsafe {
            with_qualifier(a.selector, q, q_size, |qual| {
                model::get(object, a, qual, state())
            })
        }
        .map(|v| v.size())
    };
    match size {
        Ok(s) => {
            // SAFETY: checked non-null above.
            unsafe { *out = s as u32 };
            NO_ERROR
        }
        Err(e) => {
            log(&format!(
                "size: error {} for {}",
                fourcc_str(e as u32),
                describe(object, a)
            ));
            e
        }
    }
}

unsafe extern "C" fn get_property_data(
    _d: DriverRef,
    object: AudioObjectId,
    _pid: Pid,
    addr: *const PropertyAddress,
    q_size: u32,
    q: *const c_void,
    in_size: u32,
    out_size: *mut u32,
    out: *mut c_void,
) -> OsStatus {
    // SAFETY: the HAL passes a valid address.
    let (Some(a), false, false) = (unsafe { addr.as_ref() }, out_size.is_null(), out.is_null())
    else {
        return ERR_ILLEGAL_OPERATION;
    };
    // SAFETY: qualifier pointer and size from the HAL.
    let value = unsafe {
        with_qualifier(a.selector, q, q_size, |qual| {
            model::get(object, a, qual, state())
        })
    };
    let written = value.and_then(|v| match v {
        Value::Str(s) => {
            if (in_size as usize) < size_of::<CfStringRef>() {
                return Err(ERR_BAD_PROPERTY_SIZE);
            }
            // SAFETY: valid UTF-8 bytes; the caller owns the returned +1 reference.
            let cf = unsafe {
                CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0)
            };
            if cf.is_null() {
                return Err(ERR_UNSPECIFIED);
            }
            // SAFETY: the buffer holds at least one pointer.
            unsafe { *(out as *mut CfStringRef) = cf };
            Ok(size_of::<CfStringRef>())
        }
        v => {
            // SAFETY: the HAL's buffer of `in_size` bytes.
            let buf = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, in_size as usize) };
            v.write(buf)
        }
    });
    match written {
        Ok(n) => {
            // SAFETY: checked non-null above.
            unsafe { *out_size = n as u32 };
            NO_ERROR
        }
        Err(e) => {
            log(&format!(
                "get: error {} for {}",
                fourcc_str(e as u32),
                describe(object, a)
            ));
            e
        }
    }
}

unsafe extern "C" fn set_property_data(
    _d: DriverRef,
    object: AudioObjectId,
    _pid: Pid,
    addr: *const PropertyAddress,
    _q_size: u32,
    _q: *const c_void,
    in_size: u32,
    data: *const c_void,
) -> OsStatus {
    // SAFETY: the HAL passes a valid address.
    let (Some(a), false) = (unsafe { addr.as_ref() }, data.is_null()) else {
        return ERR_ILLEGAL_OPERATION;
    };
    // SAFETY: the HAL's buffer of `in_size` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, in_size as usize) };
    match model::set(object, a, bytes) {
        Ok(Change::None) => NO_ERROR,
        Ok(Change::StreamActive(on)) => {
            STREAM_ACTIVE.store(on, Relaxed);
            NO_ERROR
        }
        Err(e) => {
            log(&format!(
                "set: error {} for {}",
                fourcc_str(e as u32),
                describe(object, a)
            ));
            e
        }
    }
}

// ---------------------------------------------------------------- IO

unsafe extern "C" fn start_io(_d: DriverRef, dev: AudioObjectId, _client: u32) -> OsStatus {
    if dev != DEVICE {
        return ERR_BAD_OBJECT;
    }
    if IO_CLIENTS.fetch_add(1, AcqRel) == 0 {
        log("start io");
    }
    NO_ERROR
}

unsafe extern "C" fn stop_io(_d: DriverRef, dev: AudioObjectId, _client: u32) -> OsStatus {
    if dev != DEVICE {
        return ERR_BAD_OBJECT;
    }
    // The HAL pairs every StopIO with a StartIO; the guard only keeps a
    // stray call from wrapping the count.
    if IO_CLIENTS.load(Acquire) == 0 {
        return NO_ERROR;
    }
    if IO_CLIENTS.fetch_sub(1, AcqRel) == 1 {
        log("stop io");
    }
    NO_ERROR
}

unsafe extern "C" fn get_zero_time_stamp(
    _d: DriverRef,
    dev: AudioObjectId,
    _client: u32,
    sample_time: *mut f64,
    host_time: *mut u64,
    seed: *mut u64,
) -> OsStatus {
    if dev != DEVICE || sample_time.is_null() || host_time.is_null() || seed.is_null() {
        return ERR_BAD_DEVICE;
    }
    let tpf = f64::from_bits(TICKS_PER_FRAME.load(Relaxed));
    // SAFETY: plain system call.
    let now = unsafe { mach_absolute_time() };
    // The timeline is fixed to host time zero, so it never changes and the
    // seed stays 1.
    let (s, h) = clock::zero_time_stamp(now, tpf);
    // SAFETY: checked non-null above.
    unsafe {
        *sample_time = s;
        *host_time = h;
        *seed = 1;
    }
    NO_ERROR
}

unsafe extern "C" fn will_do_io_operation(
    _d: DriverRef,
    dev: AudioObjectId,
    _client: u32,
    op: u32,
    will_do: *mut u8,
    in_place: *mut u8,
) -> OsStatus {
    if dev != DEVICE || will_do.is_null() || in_place.is_null() {
        return ERR_BAD_DEVICE;
    }
    // SAFETY: checked non-null above.
    unsafe {
        *will_do = (op == IO_OPERATION_READ_INPUT).into();
        *in_place = 1;
    }
    NO_ERROR
}

unsafe extern "C" fn begin_io_operation(
    _d: DriverRef,
    _dev: AudioObjectId,
    _client: u32,
    _op: u32,
    _frames: u32,
    _info: *const IoCycleInfo,
) -> OsStatus {
    NO_ERROR
}

unsafe extern "C" fn do_io_operation(
    _d: DriverRef,
    dev: AudioObjectId,
    stream: AudioObjectId,
    _client: u32,
    op: u32,
    frames: u32,
    info: *const IoCycleInfo,
    main: *mut c_void,
    _secondary: *mut c_void,
) -> OsStatus {
    if dev != DEVICE || stream != STREAM {
        return ERR_BAD_OBJECT;
    }
    if op != IO_OPERATION_READ_INPUT || main.is_null() {
        return NO_ERROR;
    }
    // SAFETY: the HAL passes valid cycle info for the operation.
    let Some(info) = (unsafe { info.as_ref() }) else {
        return ERR_ILLEGAL_OPERATION;
    };
    // SAFETY: the main buffer holds `frames` mono f32 samples in our format.
    let out = unsafe { std::slice::from_raw_parts_mut(main as *mut f32, frames as usize) };
    // The frames the client asks for at device time S were written by the
    // app at S - TRANSPORT_LATENCY.
    let pos = info.input_time.sample_time as i64 - i64::from(model::TRANSPORT_LATENCY);
    READERS.fetch_add(1, SeqCst);
    // SAFETY: published by the watcher, which keeps it alive while READERS
    // counts this reader.
    let valid = match unsafe { CURRENT.load(SeqCst).as_ref() } {
        Some(m) => m.segment().read(pos, out),
        None => {
            out.fill(0.0);
            0
        }
    };
    READERS.fetch_sub(1, SeqCst);
    MISSING.fetch_add((out.len() - valid) as u64, Relaxed);
    NO_ERROR
}

unsafe extern "C" fn end_io_operation(
    _d: DriverRef,
    _dev: AudioObjectId,
    _client: u32,
    _op: u32,
    _frames: u32,
    _info: *const IoCycleInfo,
) -> OsStatus {
    NO_ERROR
}
