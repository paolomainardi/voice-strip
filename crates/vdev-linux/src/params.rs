//! SPA parameter pods built with libspa's serializer.

use pipewire::spa;
use spa::pod::{Object, Property, Value};
use spa::sys as spa_sys;

/// `ProcessLatency` param declaring `samples` of processing latency.
pub fn process_latency(samples: usize) -> Vec<u8> {
    let obj = Object {
        type_: spa::utils::SpaTypes::ObjectParamProcessLatency.as_raw(),
        id: spa::param::ParamType::ProcessLatency.as_raw(),
        properties: vec![
            Property::new(
                spa_sys::SPA_PARAM_PROCESS_LATENCY_quantum,
                Value::Float(0.0),
            ),
            Property::new(
                spa_sys::SPA_PARAM_PROCESS_LATENCY_rate,
                Value::Int(samples.min(i32::MAX as usize) as i32),
            ),
            Property::new(spa_sys::SPA_PARAM_PROCESS_LATENCY_ns, Value::Long(0)),
        ],
    };
    serialize(Value::Object(obj))
}

/// `EnumFormat` param for mono F32 at `rate`.
pub fn mono_f32_format(rate: u32) -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(1);
    let mut position = [0u32; spa_sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa_sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let obj = Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    serialize(Value::Object(obj))
}

fn serialize(v: Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &v)
        .expect("pod serialization")
        .0
        .into_inner()
}
