//! Raises the worker thread to realtime priority so inference keeps up when
//! the machine is busy. Called once from the worker thread after the model
//! has loaded (loading is long non-blocking CPU work).
//!
//! Linux: tries `SCHED_FIFO` directly (works with realtime privileges, e.g.
//! the `realtime` group), then asks RealtimeKit over D-Bus, the
//! same service PipeWire uses on desktops. RealtimeKit requires the process
//! to have a finite `RLIMIT_RTTIME`. That limit is process-wide: it also
//! covers the PipeWire data thread and any other realtime thread, which all
//! block every cycle. It is only set (200 ms) when currently infinite, never
//! lowered, and it also guards the direct `SCHED_FIFO` path against a stuck
//! worker. The worker sleeps between frames, so it never reaches it.
//!
//! The RealtimeKit request is an in-process D-Bus call (zbus, blocking API).
//! In a Flatpak sandbox it needs `--system-talk-name=org.freedesktop.RealtimeKit1`.
//!
//! macOS: the Mach time-constraint policy, the one Core Audio's own IO
//! threads use: every 10 ms frame the worker gets up to 2 ms of computation
//! that must finish within 2.6 ms, inside the 128-sample (2.67 ms) budget
//! the neural handoff leaves it. Any process may set it on its own threads.
//!
//! macOS also gets a work interval ([`WorkInterval`]): realtime priority
//! alone does not raise the core clock, and a worker busy 0.1 ms every
//! 10 ms runs on cores the power controller keeps at their lowest speed,
//! about ten times slower. The interval tells it the deadline.
//!
//! The outcome is reported by `DfnDenoiser::priority`.

/// How the worker thread is scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// `SCHED_FIFO` set directly.
    Fifo,
    /// Realtime granted by RealtimeKit.
    RtKit,
    /// Mach time-constraint policy (macOS).
    TimeConstraint,
    /// Normal scheduling (no realtime available).
    Normal,
}

#[cfg(target_os = "linux")]
pub fn raise() -> Priority {
    const PRIO: i32 = 10;
    // SAFETY: plain libc calls on the current thread with valid arguments.
    unsafe {
        let mut old = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::getrlimit(libc::RLIMIT_RTTIME, &mut old);
        if old.rlim_max == libc::RLIM_INFINITY {
            let lim = libc::rlimit {
                rlim_cur: 200_000,
                rlim_max: 200_000,
            };
            libc::setrlimit(libc::RLIMIT_RTTIME, &lim);
        }
        let param = libc::sched_param {
            sched_priority: PRIO,
        };
        if libc::sched_setscheduler(0, libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK, &param) == 0 {
            return Priority::Fifo;
        }
        let tid = libc::gettid();
        let pid = libc::getpid();
        let ok = rtkit_make_realtime(pid as u64, tid as u64, PRIO as u32);
        let policy = libc::sched_getscheduler(0) & !libc::SCHED_RESET_ON_FORK;
        if ok && (policy == libc::SCHED_RR || policy == libc::SCHED_FIFO) {
            return Priority::RtKit;
        }
    }
    Priority::Normal
}

/// Asks RealtimeKit on the system bus to make thread `tid` of process `pid`
/// realtime at `prio`.
#[cfg(target_os = "linux")]
fn rtkit_make_realtime(pid: u64, tid: u64, prio: u32) -> bool {
    let Ok(conn) = zbus::blocking::Connection::system() else {
        return false;
    };
    conn.call_method(
        Some("org.freedesktop.RealtimeKit1"),
        "/org/freedesktop/RealtimeKit1",
        Some("org.freedesktop.RealtimeKit1"),
        "MakeThreadRealtimeWithPID",
        &(pid, tid, prio),
    )
    .is_ok()
}

#[cfg(target_os = "macos")]
pub fn raise() -> Priority {
    #[repr(C)]
    struct TimebaseInfo {
        numer: u32,
        denom: u32,
    }
    /// thread_time_constraint_policy_data_t.
    #[repr(C)]
    struct TimeConstraint {
        period: u32,
        computation: u32,
        constraint: u32,
        preemptible: i32,
    }
    const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut TimebaseInfo) -> i32;
        fn mach_thread_self() -> u32;
        fn thread_policy_set(thread: u32, flavor: u32, policy: *const i32, count: u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        static mach_task_self_: u32;
    }
    let mut tb = TimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: valid out pointer.
    unsafe { mach_timebase_info(&mut tb) };
    let ticks_per_ms = 1e6 * f64::from(tb.denom) / f64::from(tb.numer.max(1));
    let ticks = |ms: f64| (ms * ticks_per_ms) as u32;
    let policy = TimeConstraint {
        period: ticks(10.0),
        computation: ticks(2.0),
        constraint: ticks(2.6),
        preemptible: 1,
    };
    // SAFETY: plain Mach calls on the current thread; the policy struct is
    // four 32-bit words, the count thread_policy_set expects.
    let ok = unsafe {
        let thread = mach_thread_self();
        let r = thread_policy_set(
            thread,
            THREAD_TIME_CONSTRAINT_POLICY,
            &policy as *const TimeConstraint as *const i32,
            4,
        );
        mach_port_deallocate(mach_task_self_, thread);
        r == 0
    };
    if ok {
        Priority::TimeConstraint
    } else {
        Priority::Normal
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn raise() -> Priority {
    Priority::Normal
}

/// A Core Audio work interval the worker thread has joined (macOS 11 or
/// later): each frame runs between [`WorkInterval::start`] and
/// [`WorkInterval::finish`] with its deadline, so the power controller
/// raises the clock enough to meet it. Elsewhere it does nothing.
pub struct WorkInterval {
    #[cfg(target_os = "macos")]
    imp: Option<mac::Interval>,
}

impl WorkInterval {
    /// Creates an interval and joins it from the calling thread, which must
    /// be the worker (after [`raise`]). `deadline` is how long a frame may
    /// take.
    pub fn join(name: &std::ffi::CStr, deadline: std::time::Duration) -> Self {
        #[cfg(target_os = "macos")]
        {
            Self {
                imp: mac::Interval::join(name, deadline),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (name, deadline);
            Self {}
        }
    }

    /// Whether the thread joined an interval.
    pub fn joined(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.imp.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// A frame starts now.
    pub fn start(&self) {
        #[cfg(target_os = "macos")]
        if let Some(i) = &self.imp {
            i.start();
        }
    }

    /// The frame is done.
    pub fn finish(&self) {
        #[cfg(target_os = "macos")]
        if let Some(i) = &self.imp {
            i.finish();
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, c_char, c_int, c_void};

    /// os_workgroup_join_token_s is 40 bytes; more is harmless.
    #[repr(C, align(8))]
    struct Token([u8; 64]);

    const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioWorkIntervalCreate(
            name: *const c_char,
            clock: u32,
            attr: *mut c_void,
        ) -> *mut c_void;
    }
    unsafe extern "C" {
        fn os_workgroup_join(wg: *mut c_void, token: *mut Token) -> c_int;
        fn os_workgroup_leave(wg: *mut c_void, token: *mut Token);
        fn os_workgroup_interval_start(
            wg: *mut c_void,
            start: u64,
            deadline: u64,
            data: *mut c_void,
        ) -> c_int;
        fn os_workgroup_interval_finish(wg: *mut c_void, data: *mut c_void) -> c_int;
        fn os_release(object: *mut c_void);
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut [u32; 2]) -> c_int;
    }

    pub struct Interval {
        wg: *mut c_void,
        token: Box<Token>,
        /// The deadline in mach ticks.
        ticks: u64,
    }

    impl Interval {
        pub fn join(name: &CStr, deadline: std::time::Duration) -> Option<Self> {
            // SAFETY: valid C string; a null attribute means the defaults.
            let wg = unsafe {
                AudioWorkIntervalCreate(
                    name.as_ptr(),
                    OS_CLOCK_MACH_ABSOLUTE_TIME,
                    std::ptr::null_mut(),
                )
            };
            if wg.is_null() {
                return None;
            }
            let mut token = Box::new(Token([0; 64]));
            // SAFETY: a live workgroup and a token buffer that outlives the
            // membership; joined from the calling thread.
            if unsafe { os_workgroup_join(wg, &mut *token) } != 0 {
                // SAFETY: the reference AudioWorkIntervalCreate returned.
                unsafe { os_release(wg) };
                return None;
            }
            let mut tb = [0u32; 2];
            // SAFETY: valid out pointer (numer, denom).
            unsafe { mach_timebase_info(&mut tb) };
            let ticks = (deadline.as_nanos() as u64).saturating_mul(u64::from(tb[1]))
                / u64::from(tb[0].max(1));
            Some(Self { wg, token, ticks })
        }

        pub fn start(&self) {
            // SAFETY: a workgroup this thread joined; null data is allowed.
            unsafe {
                let now = mach_absolute_time();
                os_workgroup_interval_start(self.wg, now, now + self.ticks, std::ptr::null_mut());
            }
        }

        pub fn finish(&self) {
            // SAFETY: as in `start`.
            unsafe { os_workgroup_interval_finish(self.wg, std::ptr::null_mut()) };
        }
    }

    impl Drop for Interval {
        /// Dropped on the thread that joined, before it exits.
        fn drop(&mut self) {
            // SAFETY: leaves the membership made in `join`, on the same
            // thread, then drops the creation reference.
            unsafe {
                os_workgroup_leave(self.wg, &mut *self.token);
                os_release(self.wg);
            }
        }
    }
}
