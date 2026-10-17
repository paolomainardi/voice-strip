//! Flush-to-zero / denormals-are-zero for the duration of an audio callback.
//! Denormal floats are orders of magnitude slower on x86 and appear when
//! filters and envelopes decay on digital silence. The previous state is
//! restored on drop, because the host's audio thread may run other code.

pub struct FlushDenormals {
    #[cfg(target_arch = "x86_64")]
    saved: u32,
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl FlushDenormals {
    #[inline]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let mut saved: u32 = 0;
            // SAFETY: reading and writing MXCSR only changes floating-point
            // rounding/denormal behaviour of this thread.
            unsafe {
                std::arch::asm!("stmxcsr [{}]", in(reg) &mut saved, options(nostack));
                let ftz_daz = saved | (1 << 15) | (1 << 6);
                std::arch::asm!("ldmxcsr [{}]", in(reg) &ftz_daz, options(nostack));
            }
            Self { saved }
        }
        #[cfg(target_arch = "aarch64")]
        {
            let saved: u64;
            // SAFETY: FPCR.FZ (bit 24) only changes denormal handling.
            unsafe {
                std::arch::asm!("mrs {}, fpcr", out(reg) saved, options(nomem, nostack));
                std::arch::asm!("msr fpcr, {}", in(reg) saved | (1 << 24), options(nomem, nostack));
            }
            Self { saved }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self {}
        }
    }
}

impl Drop for FlushDenormals {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: restores the value read in `new`.
        unsafe {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &self.saved, options(nostack));
        }
        #[cfg(target_arch = "aarch64")]
        // SAFETY: restores the value read in `new`.
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.saved, options(nomem, nostack));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FlushDenormals;

    #[test]
    fn denormals_flush_inside_and_restore_after() {
        let tiny = std::hint::black_box(f32::MIN_POSITIVE);
        {
            let _g = FlushDenormals::new();
            let d = std::hint::black_box(tiny) * std::hint::black_box(0.5f32);
            #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
            assert_eq!(d, 0.0);
            let _ = d;
        }
        let d = std::hint::black_box(tiny) * std::hint::black_box(0.5f32);
        assert!(d > 0.0, "state not restored");
    }
}
