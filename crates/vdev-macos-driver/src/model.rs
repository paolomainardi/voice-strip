//! The object tree the driver publishes and the value of every property,
//! as plain Rust with no Core Audio calls, so it is unit tested on any host.
//!
//! Three objects: the plug-in (object 1, fixed by the HAL), one device and
//! its one input stream. 48 kHz, mono, 32-bit float, the engine's format,
//! so the HAL never converts.

use crate::hal::*;

pub const DEVICE: AudioObjectId = 2;
pub const STREAM: AudioObjectId = 3;

pub const SAMPLE_RATE: f64 = 48_000.0;
/// Frames between two zero time stamps (kAudioDevicePropertyZeroTimeStampPeriod).
pub const ZERO_TIME_STAMP_PERIOD: u32 = 16_384;
/// Frames between the app writing a frame into the shared ring and a
/// client reading it: the margin for the app's capture buffer and its
/// scheduling jitter. Reported as the device latency.
pub const TRANSPORT_LATENCY: u32 = 128;

pub const MANUFACTURER: &str = "Voice Strip";
pub const DEVICE_NAME: &str = "Voice Strip";
pub const DEVICE_UID: &str = "io.github.paolomainardi.voicestrip.device";
pub const MODEL_UID: &str = "io.github.paolomainardi.voicestrip.model";

pub const FORMAT: StreamBasicDescription = StreamBasicDescription {
    sample_rate: SAMPLE_RATE,
    format_id: FORMAT_LINEAR_PCM,
    format_flags: FORMAT_FLAGS_NATIVE_FLOAT_PACKED,
    bytes_per_packet: 4,
    frames_per_packet: 1,
    bytes_per_frame: 4,
    channels_per_frame: 1,
    bits_per_channel: 32,
    reserved: 0,
};

const RATE_RANGE: ValueRange = ValueRange {
    minimum: SAMPLE_RATE,
    maximum: SAMPLE_RATE,
};

/// Runtime state the property values depend on.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub running: bool,
    pub stream_active: bool,
}

/// Qualifier data of a property query, already decoded by the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct Qualifier<'a> {
    /// kAudioObjectPropertyOwnedObjects: classes to keep (empty keeps all).
    pub classes: &'a [u32],
    /// kAudioPlugInPropertyTranslateUIDToDevice: the UID to look up.
    pub uid: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    U32(u32),
    F64(f64),
    /// An array of AudioObjectIDs or UInt32s.
    Ids(Vec<u32>),
    /// A CFStringRef; the caller creates it.
    Str(&'static str),
    Format(StreamBasicDescription),
    Rates(Vec<ValueRange>),
    /// AudioStreamRangedDescription list.
    Formats(Vec<(StreamBasicDescription, ValueRange)>),
    /// An AudioChannelLayout with one mono channel description.
    MonoLayout,
}

const MONO_LAYOUT_SIZE: usize = 12 + 20;
const RANGED_FORMAT_SIZE: usize = 40 + 16;

impl Value {
    /// Bytes the value takes in a GetPropertyData buffer.
    pub fn size(&self) -> usize {
        match self {
            Value::U32(_) => 4,
            Value::F64(_) => 8,
            Value::Ids(v) => 4 * v.len(),
            Value::Str(_) => size_of::<usize>(),
            Value::Format(_) => 40,
            Value::Rates(v) => 16 * v.len(),
            Value::Formats(v) => RANGED_FORMAT_SIZE * v.len(),
            Value::MonoLayout => MONO_LAYOUT_SIZE,
        }
    }

    /// Writes the value into `out`, returning the bytes written. Arrays are
    /// cut to whole items that fit, as the HAL expects; a scalar that does
    /// not fit is an error. `Str` is written by the caller.
    pub fn write(&self, out: &mut [u8]) -> Result<usize, OsStatus> {
        let mut w = Writer { out, at: 0 };
        match self {
            Value::U32(v) => w.whole(4, |w| w.u32(*v))?,
            Value::F64(v) => w.whole(8, |w| w.f64(*v))?,
            Value::Ids(v) => v.iter().take(w.fits(4)).for_each(|v| w.u32(*v)),
            Value::Str(_) => return Err(ERR_UNSPECIFIED),
            Value::Format(f) => w.whole(40, |w| w.format(f))?,
            Value::Rates(v) => v.iter().take(w.fits(16)).for_each(|r| w.range(r)),
            Value::Formats(v) => v
                .iter()
                .take(w.fits(RANGED_FORMAT_SIZE))
                .for_each(|(f, r)| {
                    w.format(f);
                    w.range(r);
                }),
            Value::MonoLayout => w.whole(MONO_LAYOUT_SIZE, |w| {
                // mChannelLayoutTag UseChannelDescriptions, mChannelBitmap,
                // mNumberChannelDescriptions, then mChannelLabel,
                // mChannelFlags and mCoordinates[3].
                for v in [0, 0, 1, CHANNEL_LABEL_MONO, 0] {
                    w.u32(v);
                }
                for _ in 0..3 {
                    w.f32(0.0);
                }
            })?,
        }
        Ok(w.at)
    }
}

struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
}

impl Writer<'_> {
    fn fits(&self, item: usize) -> usize {
        self.out.len() / item
    }
    fn whole(&mut self, size: usize, f: impl FnOnce(&mut Self)) -> Result<(), OsStatus> {
        if self.out.len() < size {
            return Err(ERR_BAD_PROPERTY_SIZE);
        }
        f(self);
        Ok(())
    }
    fn bytes(&mut self, b: &[u8]) {
        self.out[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_ne_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.bytes(&v.to_ne_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.bytes(&v.to_ne_bytes());
    }
    fn range(&mut self, r: &ValueRange) {
        self.f64(r.minimum);
        self.f64(r.maximum);
    }
    fn format(&mut self, f: &StreamBasicDescription) {
        self.f64(f.sample_rate);
        for v in [
            f.format_id,
            f.format_flags,
            f.bytes_per_packet,
            f.frames_per_packet,
            f.bytes_per_frame,
            f.channels_per_frame,
            f.bits_per_channel,
            f.reserved,
        ] {
            self.u32(v);
        }
    }
}

fn class_of(object: AudioObjectId) -> Option<u32> {
    match object {
        OBJECT_PLUGIN => Some(CLASS_PLUGIN),
        DEVICE => Some(CLASS_DEVICE),
        STREAM => Some(CLASS_STREAM),
        _ => None,
    }
}

fn owned(objects: &[AudioObjectId], q: &Qualifier) -> Value {
    Value::Ids(
        objects
            .iter()
            .copied()
            .filter(|o| {
                q.classes.is_empty() || class_of(*o).is_some_and(|c| q.classes.contains(&c))
            })
            .collect(),
    )
}

/// The device's streams in a scope: the one input stream, except on the
/// output side.
fn streams_in(scope: u32) -> Vec<AudioObjectId> {
    if scope == SCOPE_OUTPUT {
        vec![]
    } else {
        vec![STREAM]
    }
}

/// Whether `object` has the property at `addr`.
pub fn has(object: AudioObjectId, addr: &PropertyAddress) -> bool {
    get(object, addr, &Qualifier::default(), State::default()).is_ok()
        || (object == OBJECT_PLUGIN && addr.selector == PROP_TRANSLATE_UID_TO_DEVICE)
}

/// The value of the property at `addr` of `object`.
pub fn get(
    object: AudioObjectId,
    addr: &PropertyAddress,
    q: &Qualifier,
    st: State,
) -> Result<Value, OsStatus> {
    use Value::*;
    let v = match (object, addr.selector) {
        (_, PROP_BASE_CLASS) if class_of(object).is_some() => U32(CLASS_OBJECT),
        (_, PROP_CLASS) => U32(class_of(object).ok_or(ERR_BAD_OBJECT)?),
        (_, PROP_CUSTOM_PROPERTY_INFO_LIST) if class_of(object).is_some() => Ids(vec![]),

        (OBJECT_PLUGIN, PROP_OWNER) => U32(OBJECT_UNKNOWN),
        (OBJECT_PLUGIN, PROP_MANUFACTURER) => Str(MANUFACTURER),
        (OBJECT_PLUGIN, PROP_OWNED_OBJECTS) => owned(&[DEVICE], q),
        (OBJECT_PLUGIN, PROP_DEVICE_LIST) => Ids(vec![DEVICE]),
        (OBJECT_PLUGIN, PROP_BOX_LIST) => Ids(vec![]),
        (OBJECT_PLUGIN, PROP_TRANSLATE_UID_TO_DEVICE) => match q.uid {
            Some(DEVICE_UID) => U32(DEVICE),
            Some(_) => U32(OBJECT_UNKNOWN),
            None => return Err(ERR_BAD_PROPERTY_SIZE),
        },
        (OBJECT_PLUGIN, PROP_RESOURCE_BUNDLE) => Str(""),

        (DEVICE, PROP_OWNER) => U32(OBJECT_PLUGIN),
        (DEVICE, PROP_NAME) => Str(DEVICE_NAME),
        (DEVICE, PROP_MANUFACTURER) => Str(MANUFACTURER),
        (DEVICE, PROP_OWNED_OBJECTS) => owned(&streams_in(addr.scope), q),
        (DEVICE, PROP_DEVICE_UID) => Str(DEVICE_UID),
        (DEVICE, PROP_MODEL_UID) => Str(MODEL_UID),
        (DEVICE, PROP_TRANSPORT_TYPE) => U32(TRANSPORT_TYPE_VIRTUAL),
        (DEVICE, PROP_RELATED_DEVICES) => Ids(vec![DEVICE]),
        (DEVICE, PROP_CLOCK_DOMAIN) => U32(0),
        (DEVICE, PROP_DEVICE_IS_ALIVE) => U32(1),
        (DEVICE, PROP_DEVICE_IS_RUNNING) => U32(st.running.into()),
        (DEVICE, PROP_CAN_BE_DEFAULT_DEVICE) => U32(1),
        // The system device plays alerts: never a microphone.
        (DEVICE, PROP_CAN_BE_DEFAULT_SYSTEM_DEVICE) => U32(0),
        (DEVICE, PROP_LATENCY) => U32(TRANSPORT_LATENCY),
        (DEVICE, PROP_STREAMS) => Ids(streams_in(addr.scope)),
        (DEVICE, PROP_CONTROL_LIST) => Ids(vec![]),
        (DEVICE, PROP_SAFETY_OFFSET) => U32(0),
        (DEVICE, PROP_NOMINAL_SAMPLE_RATE) => F64(SAMPLE_RATE),
        (DEVICE, PROP_AVAILABLE_NOMINAL_SAMPLE_RATES) => Rates(vec![RATE_RANGE]),
        (DEVICE, PROP_IS_HIDDEN) => U32(0),
        (DEVICE, PROP_ZERO_TIME_STAMP_PERIOD) => U32(ZERO_TIME_STAMP_PERIOD),
        (DEVICE, PROP_CLOCK_IS_STABLE) => U32(1),
        (DEVICE, PROP_CLOCK_ALGORITHM) => U32(CLOCK_ALGORITHM_RAW),
        (DEVICE, PROP_PREFERRED_CHANNELS_FOR_STEREO) => Ids(vec![1, 1]),
        (DEVICE, PROP_PREFERRED_CHANNEL_LAYOUT) => MonoLayout,

        (STREAM, PROP_OWNER) => U32(DEVICE),
        (STREAM, PROP_OWNED_OBJECTS) => Ids(vec![]),
        (STREAM, PROP_STREAM_IS_ACTIVE) => U32(st.stream_active.into()),
        // 1: input.
        (STREAM, PROP_STREAM_DIRECTION) => U32(1),
        (STREAM, PROP_STREAM_TERMINAL_TYPE) => U32(TERMINAL_TYPE_MICROPHONE),
        (STREAM, PROP_STREAM_STARTING_CHANNEL) => U32(1),
        (STREAM, PROP_LATENCY) => U32(0),
        (STREAM, PROP_STREAM_VIRTUAL_FORMAT | PROP_STREAM_PHYSICAL_FORMAT) => Format(FORMAT),
        (
            STREAM,
            PROP_STREAM_AVAILABLE_VIRTUAL_FORMATS | PROP_STREAM_AVAILABLE_PHYSICAL_FORMATS,
        ) => Formats(vec![(FORMAT, RATE_RANGE)]),

        _ if class_of(object).is_none() => return Err(ERR_BAD_OBJECT),
        _ => return Err(ERR_UNKNOWN_PROPERTY),
    };
    Ok(v)
}

pub fn is_settable(object: AudioObjectId, addr: &PropertyAddress) -> Result<bool, OsStatus> {
    if !has(object, addr) {
        return Err(if class_of(object).is_none() {
            ERR_BAD_OBJECT
        } else {
            ERR_UNKNOWN_PROPERTY
        });
    }
    Ok(matches!(
        (object, addr.selector),
        (DEVICE, PROP_NOMINAL_SAMPLE_RATE)
            | (STREAM, PROP_STREAM_IS_ACTIVE)
            | (STREAM, PROP_STREAM_VIRTUAL_FORMAT)
            | (STREAM, PROP_STREAM_PHYSICAL_FORMAT)
    ))
}

/// What a SetPropertyData call changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    /// Accepted, nothing to do: the value is the only one supported.
    None,
    StreamActive(bool),
}

/// Validates a SetPropertyData call. Only the one supported rate and format
/// are accepted.
pub fn set(object: AudioObjectId, addr: &PropertyAddress, data: &[u8]) -> Result<Change, OsStatus> {
    if !is_settable(object, addr)? {
        return Err(ERR_UNSUPPORTED_OPERATION);
    }
    match addr.selector {
        PROP_NOMINAL_SAMPLE_RATE => {
            let b: [u8; 8] = data
                .get(..8)
                .ok_or(ERR_BAD_PROPERTY_SIZE)?
                .try_into()
                .unwrap();
            if f64::from_ne_bytes(b) == SAMPLE_RATE {
                Ok(Change::None)
            } else {
                Err(ERR_UNSUPPORTED_FORMAT)
            }
        }
        PROP_STREAM_IS_ACTIVE => {
            let b: [u8; 4] = data
                .get(..4)
                .ok_or(ERR_BAD_PROPERTY_SIZE)?
                .try_into()
                .unwrap();
            Ok(Change::StreamActive(u32::from_ne_bytes(b) != 0))
        }
        _ => {
            let mut expected = [0u8; 40];
            Value::Format(FORMAT).write(&mut expected)?;
            if data.get(..40).ok_or(ERR_BAD_PROPERTY_SIZE)? == expected {
                Ok(Change::None)
            } else {
                Err(ERR_UNSUPPORTED_FORMAT)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(selector: u32) -> PropertyAddress {
        scoped(selector, SCOPE_GLOBAL)
    }

    fn scoped(selector: u32, scope: u32) -> PropertyAddress {
        PropertyAddress {
            selector,
            scope,
            element: ELEMENT_MAIN,
        }
    }

    fn value(object: AudioObjectId, a: PropertyAddress) -> Value {
        get(object, &a, &Qualifier::default(), State::default()).unwrap()
    }

    #[test]
    fn plugin_publishes_one_device() {
        assert_eq!(
            value(OBJECT_PLUGIN, addr(PROP_DEVICE_LIST)),
            Value::Ids(vec![DEVICE])
        );
        assert_eq!(
            value(OBJECT_PLUGIN, addr(PROP_CLASS)),
            Value::U32(CLASS_PLUGIN)
        );
        let q = Qualifier {
            uid: Some(DEVICE_UID),
            ..Default::default()
        };
        let uidd = addr(PROP_TRANSLATE_UID_TO_DEVICE);
        assert!(has(OBJECT_PLUGIN, &uidd));
        assert_eq!(
            get(OBJECT_PLUGIN, &uidd, &q, State::default()),
            Ok(Value::U32(DEVICE))
        );
    }

    #[test]
    fn device_is_input_only() {
        assert_eq!(
            value(DEVICE, scoped(PROP_STREAMS, SCOPE_INPUT)),
            Value::Ids(vec![STREAM])
        );
        assert_eq!(
            value(DEVICE, scoped(PROP_STREAMS, SCOPE_OUTPUT)),
            Value::Ids(vec![])
        );
        assert_eq!(value(STREAM, addr(PROP_STREAM_DIRECTION)), Value::U32(1));
    }

    #[test]
    fn owned_objects_filter_by_class() {
        let a = addr(PROP_OWNED_OBJECTS);
        let streams = Qualifier {
            classes: &[CLASS_STREAM],
            ..Default::default()
        };
        let devices = Qualifier {
            classes: &[CLASS_DEVICE],
            ..Default::default()
        };
        assert_eq!(
            get(DEVICE, &a, &streams, State::default()),
            Ok(Value::Ids(vec![STREAM]))
        );
        assert_eq!(
            get(DEVICE, &a, &devices, State::default()),
            Ok(Value::Ids(vec![]))
        );
    }

    #[test]
    fn unknown_objects_and_properties_are_errors() {
        assert_eq!(
            get(
                9,
                &addr(PROP_CLASS),
                &Qualifier::default(),
                State::default()
            ),
            Err(ERR_BAD_OBJECT)
        );
        assert_eq!(
            get(
                DEVICE,
                &addr(fourcc(b"zzzz")),
                &Qualifier::default(),
                State::default()
            ),
            Err(ERR_UNKNOWN_PROPERTY)
        );
        assert!(!has(STREAM, &addr(PROP_DEVICE_UID)));
    }

    #[test]
    fn values_serialize_with_their_declared_size() {
        let mut buf = [0u8; 256];
        for (object, a) in [
            (DEVICE, addr(PROP_NOMINAL_SAMPLE_RATE)),
            (DEVICE, addr(PROP_AVAILABLE_NOMINAL_SAMPLE_RATES)),
            (DEVICE, addr(PROP_PREFERRED_CHANNEL_LAYOUT)),
            (STREAM, addr(PROP_STREAM_VIRTUAL_FORMAT)),
            (STREAM, addr(PROP_STREAM_AVAILABLE_PHYSICAL_FORMATS)),
        ] {
            let v = value(object, a);
            assert_eq!(v.write(&mut buf), Ok(v.size()), "{a:?}");
        }
        assert_eq!(
            Value::U32(1).write(&mut buf[..2]),
            Err(ERR_BAD_PROPERTY_SIZE)
        );
        // Arrays are cut to the items that fit.
        assert_eq!(Value::Ids(vec![1, 2, 3]).write(&mut buf[..9]), Ok(8));
    }

    #[test]
    fn only_the_native_format_and_rate_are_accepted() {
        let mut fmt = [0u8; 40];
        Value::Format(FORMAT).write(&mut fmt).unwrap();
        let sfmt = addr(PROP_STREAM_VIRTUAL_FORMAT);
        assert_eq!(set(STREAM, &sfmt, &fmt), Ok(Change::None));
        fmt[0..8].copy_from_slice(&44_100f64.to_ne_bytes());
        assert_eq!(set(STREAM, &sfmt, &fmt), Err(ERR_UNSUPPORTED_FORMAT));
        let nsrt = addr(PROP_NOMINAL_SAMPLE_RATE);
        assert_eq!(
            set(DEVICE, &nsrt, &48_000f64.to_ne_bytes()),
            Ok(Change::None)
        );
        let sact = addr(PROP_STREAM_IS_ACTIVE);
        assert_eq!(
            set(STREAM, &sact, &0u32.to_ne_bytes()),
            Ok(Change::StreamActive(false))
        );
        assert_eq!(
            set(DEVICE, &addr(PROP_NAME), &[]),
            Err(ERR_UNSUPPORTED_OPERATION)
        );
    }
}
