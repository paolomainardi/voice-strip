//! Core Audio HAL constants and plain-data types used by the driver, copied
//! from the macOS SDK headers (`AudioHardwareBase.h`, `AudioHardware.h`,
//! `AudioServerPlugIn.h`, `CoreAudioBaseTypes.h`). Only what the driver uses.

/// A four-character code as the HAL spells it, `'clas'` in C.
pub const fn fourcc(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}

pub type AudioObjectId = u32;
pub type OsStatus = i32;

// Predefined objects.
pub const OBJECT_UNKNOWN: AudioObjectId = 0;
pub const OBJECT_PLUGIN: AudioObjectId = 1;

// Classes.
pub const CLASS_OBJECT: u32 = fourcc(b"aobj");
pub const CLASS_PLUGIN: u32 = fourcc(b"aplg");
pub const CLASS_DEVICE: u32 = fourcc(b"adev");
pub const CLASS_STREAM: u32 = fourcc(b"astr");

// Scopes and elements.
pub const SCOPE_GLOBAL: u32 = fourcc(b"glob");
pub const SCOPE_INPUT: u32 = fourcc(b"inpt");
pub const SCOPE_OUTPUT: u32 = fourcc(b"outp");
pub const ELEMENT_MAIN: u32 = 0;

// AudioObject properties.
pub const PROP_CLASS: u32 = fourcc(b"clas");
pub const PROP_BASE_CLASS: u32 = fourcc(b"bcls");
pub const PROP_OWNER: u32 = fourcc(b"stdv");
pub const PROP_OWNED_OBJECTS: u32 = fourcc(b"ownd");
pub const PROP_NAME: u32 = fourcc(b"lnam");
pub const PROP_MANUFACTURER: u32 = fourcc(b"lmak");
pub const PROP_CUSTOM_PROPERTY_INFO_LIST: u32 = fourcc(b"cust");
pub const PROP_CONTROL_LIST: u32 = fourcc(b"ctrl");

// AudioPlugIn properties.
pub const PROP_DEVICE_LIST: u32 = fourcc(b"dev#");
pub const PROP_BOX_LIST: u32 = fourcc(b"box#");
pub const PROP_TRANSLATE_UID_TO_DEVICE: u32 = fourcc(b"uidd");
pub const PROP_RESOURCE_BUNDLE: u32 = fourcc(b"rsrc");

// AudioDevice properties.
pub const PROP_DEVICE_UID: u32 = fourcc(b"uid ");
pub const PROP_MODEL_UID: u32 = fourcc(b"muid");
pub const PROP_TRANSPORT_TYPE: u32 = fourcc(b"tran");
pub const PROP_RELATED_DEVICES: u32 = fourcc(b"akin");
pub const PROP_CLOCK_DOMAIN: u32 = fourcc(b"clkd");
pub const PROP_DEVICE_IS_ALIVE: u32 = fourcc(b"livn");
pub const PROP_DEVICE_IS_RUNNING: u32 = fourcc(b"goin");
pub const PROP_CAN_BE_DEFAULT_DEVICE: u32 = fourcc(b"dflt");
pub const PROP_CAN_BE_DEFAULT_SYSTEM_DEVICE: u32 = fourcc(b"sflt");
pub const PROP_LATENCY: u32 = fourcc(b"ltnc");
pub const PROP_STREAMS: u32 = fourcc(b"stm#");
pub const PROP_SAFETY_OFFSET: u32 = fourcc(b"saft");
pub const PROP_NOMINAL_SAMPLE_RATE: u32 = fourcc(b"nsrt");
pub const PROP_AVAILABLE_NOMINAL_SAMPLE_RATES: u32 = fourcc(b"nsr#");
pub const PROP_IS_HIDDEN: u32 = fourcc(b"hidn");
pub const PROP_ZERO_TIME_STAMP_PERIOD: u32 = fourcc(b"ring");
pub const PROP_CLOCK_IS_STABLE: u32 = fourcc(b"cstb");
pub const PROP_CLOCK_ALGORITHM: u32 = fourcc(b"clok");
pub const PROP_PREFERRED_CHANNELS_FOR_STEREO: u32 = fourcc(b"dch2");
pub const PROP_PREFERRED_CHANNEL_LAYOUT: u32 = fourcc(b"srnd");

// AudioStream properties. kAudioStreamPropertyLatency is the device's 'ltnc'.
pub const PROP_STREAM_IS_ACTIVE: u32 = fourcc(b"sact");
pub const PROP_STREAM_DIRECTION: u32 = fourcc(b"sdir");
pub const PROP_STREAM_TERMINAL_TYPE: u32 = fourcc(b"term");
pub const PROP_STREAM_STARTING_CHANNEL: u32 = fourcc(b"schn");
pub const PROP_STREAM_VIRTUAL_FORMAT: u32 = fourcc(b"sfmt");
pub const PROP_STREAM_AVAILABLE_VIRTUAL_FORMATS: u32 = fourcc(b"sfma");
pub const PROP_STREAM_PHYSICAL_FORMAT: u32 = fourcc(b"pft ");
pub const PROP_STREAM_AVAILABLE_PHYSICAL_FORMATS: u32 = fourcc(b"pfta");

// Property values.
pub const TRANSPORT_TYPE_VIRTUAL: u32 = fourcc(b"virt");
pub const TERMINAL_TYPE_MICROPHONE: u32 = fourcc(b"micr");
pub const CLOCK_ALGORITHM_RAW: u32 = fourcc(b"raww");
/// kAudioChannelLabel_Mono.
pub const CHANNEL_LABEL_MONO: u32 = 42;
pub const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
/// kAudioFormatFlagsNativeFloatPacked on a little-endian host.
pub const FORMAT_FLAGS_NATIVE_FLOAT_PACKED: u32 = 0x1 | 0x8;

// Errors.
pub const NO_ERROR: OsStatus = 0;
pub const ERR_UNSPECIFIED: OsStatus = fourcc(b"what") as OsStatus;
pub const ERR_UNKNOWN_PROPERTY: OsStatus = fourcc(b"who?") as OsStatus;
pub const ERR_BAD_PROPERTY_SIZE: OsStatus = fourcc(b"!siz") as OsStatus;
pub const ERR_ILLEGAL_OPERATION: OsStatus = fourcc(b"nope") as OsStatus;
pub const ERR_BAD_OBJECT: OsStatus = fourcc(b"!obj") as OsStatus;
pub const ERR_BAD_DEVICE: OsStatus = fourcc(b"!dev") as OsStatus;
pub const ERR_UNSUPPORTED_OPERATION: OsStatus = fourcc(b"unop") as OsStatus;
pub const ERR_UNSUPPORTED_FORMAT: OsStatus = fourcc(b"!dat") as OsStatus;

// IO operations.
pub const IO_OPERATION_READ_INPUT: u32 = fourcc(b"read");

/// AudioObjectPropertyAddress.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyAddress {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

/// AudioStreamBasicDescription.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamBasicDescription {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub bytes_per_frame: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
    pub reserved: u32,
}

/// AudioValueRange.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueRange {
    pub minimum: f64,
    pub maximum: f64,
}

/// SMPTETime.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SmpteTime {
    pub subframes: i16,
    pub subframe_divisor: i16,
    pub counter: u32,
    pub kind: u32,
    pub flags: u32,
    pub hours: i16,
    pub minutes: i16,
    pub seconds: i16,
    pub frames: i16,
}

/// AudioTimeStamp.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeStamp {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
    pub word_clock_time: u64,
    pub smpte_time: SmpteTime,
    pub flags: u32,
    pub reserved: u32,
}

/// AudioServerPlugInIOCycleInfo.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IoCycleInfo {
    pub io_cycle_counter: u64,
    pub nominal_io_buffer_frame_size: u32,
    pub current_time: TimeStamp,
    pub input_time: TimeStamp,
    pub output_time: TimeStamp,
    pub main_host_ticks_per_frame: f64,
    pub device_host_ticks_per_frame: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_matches_the_sdk() {
        // 'dev#' == 0x64657623 in objc2-core-audio's generated bindings.
        assert_eq!(PROP_DEVICE_LIST, 0x6465_7623);
        assert_eq!(PROP_DEVICE_UID, 0x7569_6420);
    }

    #[test]
    fn layouts_match_the_sdk() {
        assert_eq!(size_of::<PropertyAddress>(), 12);
        assert_eq!(size_of::<StreamBasicDescription>(), 40);
        assert_eq!(size_of::<SmpteTime>(), 24);
        assert_eq!(size_of::<TimeStamp>(), 64);
        assert_eq!(size_of::<IoCycleInfo>(), 8 + 8 + 3 * 64 + 16);
    }
}
