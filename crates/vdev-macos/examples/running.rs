//! Prints, twice a second, whether the Voice Strip device runs IO: the
//! driver's own answer (`goin`) and Core Audio's across clients (`gone`).
//! A debugging aid for standby: run it while an application records.

#[cfg(target_os = "macos")]
fn main() {
    use std::ffi::c_void;
    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }
    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            id: u32,
            addr: *const Address,
            qsize: u32,
            qdata: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }
    let fourcc = |s: &[u8; 4]| u32::from_be_bytes(*s);
    let dev = vdev_macos::hal::find(vdev_macos_driver::model::DEVICE_UID)
        .ok()
        .flatten()
        .expect("Voice Strip device not found");
    let get = |sel: &[u8; 4]| {
        let addr = Address {
            selector: fourcc(sel),
            scope: fourcc(b"glob"),
            element: 0,
        };
        let mut v = 0u32;
        let mut size = 4u32;
        // SAFETY: valid address and a u32 out value.
        let r = unsafe {
            AudioObjectGetPropertyData(
                dev,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                &mut v as *mut u32 as *mut c_void,
            )
        };
        if r == 0 {
            v.to_string()
        } else {
            format!("err {r}")
        }
    };
    for _ in 0..40 {
        println!("goin {}  gone {}", get(b"goin"), get(b"gone"));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
