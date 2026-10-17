//! Registry bookkeeping: nodes, ports, default devices, and one-shot device
//! listing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use engine::{BackendError, DeviceInfo};
use pipewire as pw;
use pw::registry::GlobalObject;
use pw::spa::utils::dict::DictRef;
use pw::types::ObjectType;

use crate::{DSP_NODE_NAME, VIRTUAL_MIC_NAME};

#[derive(Clone, Debug)]
pub struct NodeEntry {
    pub name: String,
    pub description: String,
    pub class: String,
}

#[derive(Clone, Debug)]
pub struct PortEntry {
    pub node_id: u32,
    pub output: bool,
    pub name: String,
    pub channel: String,
    pub monitor: bool,
    /// `port.id`, the index within the node.
    pub index: u32,
}

#[derive(Default)]
pub struct Graph {
    pub nodes: HashMap<u32, NodeEntry>,
    pub ports: HashMap<u32, PortEntry>,
    pub default_source: Option<String>,
    pub default_sink: Option<String>,
}

impl Graph {
    pub fn add_global(&mut self, g: &GlobalObject<&DictRef>) -> bool {
        let Some(props) = g.props else {
            return false;
        };
        match g.type_ {
            ObjectType::Node => {
                let name = props.get("node.name").unwrap_or_default().to_owned();
                let description = props
                    .get("node.description")
                    .or_else(|| props.get("node.nick"))
                    .unwrap_or(&name)
                    .to_owned();
                let class = props.get("media.class").unwrap_or_default().to_owned();
                self.nodes.insert(
                    g.id,
                    NodeEntry {
                        name,
                        description,
                        class,
                    },
                );
                true
            }
            ObjectType::Port => {
                let Some(node_id) = props.get("node.id").and_then(|s| s.parse().ok()) else {
                    return false;
                };
                self.ports.insert(
                    g.id,
                    PortEntry {
                        node_id,
                        output: props.get("port.direction") == Some("out"),
                        name: props.get("port.name").unwrap_or_default().to_owned(),
                        channel: props.get("audio.channel").unwrap_or_default().to_owned(),
                        monitor: props.get("port.monitor") == Some("true"),
                        index: props
                            .get("port.id")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0),
                    },
                );
                true
            }
            _ => false,
        }
    }

    pub fn remove_global(&mut self, id: u32) -> bool {
        self.nodes.remove(&id).is_some() | self.ports.remove(&id).is_some()
    }

    pub fn node_by_name(&self, name: &str) -> Option<u32> {
        self.nodes
            .iter()
            .find(|(_, n)| n.name == name)
            .map(|(id, _)| *id)
    }

    /// Output port of `node` carrying the mono or left channel (lowest
    /// index otherwise). Monitor ports count when the node has no other
    /// outputs (null sinks used as sources).
    pub fn capture_port(&self, node: u32) -> Option<u32> {
        let mut candidates: Vec<(&u32, &PortEntry)> = self
            .ports
            .iter()
            .filter(|(_, p)| p.node_id == node && p.output)
            .collect();
        if candidates.iter().any(|(_, p)| !p.monitor) {
            candidates.retain(|(_, p)| !p.monitor);
        }
        candidates.sort_by_key(|(_, p)| (p.channel != "MONO", p.channel != "FL", p.index));
        candidates.first().map(|(id, _)| **id)
    }

    /// Monitor output port of `node` with the lowest index.
    pub fn monitor_port(&self, node: u32) -> Option<u32> {
        self.ports
            .iter()
            .filter(|(_, p)| p.node_id == node && p.output && p.monitor)
            .min_by_key(|(_, p)| p.index)
            .map(|(id, _)| *id)
    }

    /// Input port of `node` with the lowest index (a capture stream's port).
    pub fn capture_in_port(&self, node: u32) -> Option<u32> {
        self.ports
            .iter()
            .filter(|(_, p)| p.node_id == node && !p.output)
            .min_by_key(|(_, p)| p.index)
            .map(|(id, _)| *id)
    }

    /// Playback input ports of `node`, all channels.
    pub fn playback_ports(&self, node: u32) -> Vec<u32> {
        let mut v: Vec<(u32, u32)> = self
            .ports
            .iter()
            .filter(|(_, p)| p.node_id == node && !p.output)
            .map(|(id, p)| (p.index, *id))
            .collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// Input port named `name` on `node`.
    pub fn port_named(&self, node: u32, name: &str, output: bool) -> Option<u32> {
        self.ports
            .iter()
            .find(|(_, p)| p.node_id == node && p.output == output && p.name == name)
            .map(|(id, _)| *id)
    }

    pub fn set_default(&mut self, key: &str, value: Option<&str>) -> bool {
        let name = value.and_then(json_name);
        match key {
            "default.audio.source" => {
                self.default_source = name;
                true
            }
            "default.audio.sink" => {
                self.default_sink = name;
                true
            }
            _ => false,
        }
    }
}

/// Extracts `name` from the metadata value `{"name":"..."}`.
fn json_name(v: &str) -> Option<String> {
    let i = v.find("\"name\"")?;
    let rest = &v[i + 6..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_owned())
}

/// Lists `Audio/Source*` (`inputs`) or `Audio/Sink*` nodes with a one-shot
/// connection. The virtual microphone itself is excluded.
pub fn list_devices(inputs: bool) -> Result<Vec<DeviceInfo>, BackendError> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(err)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(err)?;
    let core = context.connect_rc(None).map_err(err)?;
    let registry = core.get_registry_rc().map_err(err)?;
    let graph = Rc::new(RefCell::new(Graph::default()));
    let _listener = {
        let graph = graph.clone();
        registry
            .add_listener_local()
            .global(move |g| {
                graph.borrow_mut().add_global(g);
            })
            .register()
    };
    roundtrip(&mainloop, &core)?;
    let prefix = if inputs { "Audio/Source" } else { "Audio/Sink" };
    let graph = graph.borrow();
    let mut list: Vec<DeviceInfo> = graph
        .nodes
        .values()
        .filter(|n| {
            (n.class == prefix || n.class.starts_with(&format!("{prefix}/")))
                && !n.class.ends_with("/Internal")
                && n.name != VIRTUAL_MIC_NAME
                && n.name != DSP_NODE_NAME
        })
        .map(|n| DeviceInfo {
            id: n.name.clone(),
            label: n.description.clone(),
        })
        .collect();
    list.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(list)
}

pub fn roundtrip(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::CoreRc,
) -> Result<(), BackendError> {
    let done = Rc::new(Cell::new(false));
    let pending = core.sync(0).map_err(err)?;
    let _l = {
        let done_d = done.clone();
        let loop_d = mainloop.clone();
        let done_e = done.clone();
        let loop_e = mainloop.clone();
        core.add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done_d.set(true);
                    loop_d.quit();
                }
            })
            .error(move |_, _, _, _| {
                done_e.set(true);
                loop_e.quit();
            })
            .register()
    };
    while !done.get() {
        mainloop.run();
    }
    Ok(())
}

pub fn err(e: impl std::fmt::Display) -> BackendError {
    BackendError(e.to_string())
}
