//! Packet names by phase, direction and id, read at run time from the `packets.json` report
//! produced by the official generator (never copied into the repository).

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl Phase {
    pub fn report_key(self) -> &'static str {
        match self {
            Phase::Handshake => "handshake",
            Phase::Status => "status",
            Phase::Login => "login",
            Phase::Configuration => "configuration",
            Phase::Play => "play",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    /// Server to client.
    ToClient,
    /// Client to server.
    ToServer,
}

impl Dir {
    pub fn report_key(self) -> &'static str {
        match self {
            Dir::ToClient => "clientbound",
            Dir::ToServer => "serverbound",
        }
    }

    pub fn arrow(self) -> &'static str {
        match self {
            Dir::ToClient => "S→C",
            Dir::ToServer => "C→S",
        }
    }
}

/// Packets that change the connection's state. The keys are those of the report; their numeric
/// ids are resolved at load time, never hard-coded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Intention,
    LoginCompression,
    LoginFinished,
    LoginAcknowledged,
    FinishConfigurationToClient,
    FinishConfigurationToServer,
    StartConfiguration,
    ConfigurationAcknowledged,
}

const TRANSITIONS: &[(Transition, Phase, Dir, &str)] = &[
    (Transition::Intention, Phase::Handshake, Dir::ToServer, "minecraft:intention"),
    (Transition::LoginCompression, Phase::Login, Dir::ToClient, "minecraft:login_compression"),
    (Transition::LoginFinished, Phase::Login, Dir::ToClient, "minecraft:login_finished"),
    (Transition::LoginAcknowledged, Phase::Login, Dir::ToServer, "minecraft:login_acknowledged"),
    (Transition::FinishConfigurationToClient, Phase::Configuration, Dir::ToClient, "minecraft:finish_configuration"),
    (Transition::FinishConfigurationToServer, Phase::Configuration, Dir::ToServer, "minecraft:finish_configuration"),
    (Transition::StartConfiguration, Phase::Play, Dir::ToClient, "minecraft:start_configuration"),
    (Transition::ConfigurationAcknowledged, Phase::Play, Dir::ToServer, "minecraft:configuration_acknowledged"),
];

pub struct Catalog {
    names: HashMap<(Phase, Dir, i32), String>,
    transitions: HashMap<(Phase, Dir, i32), Transition>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut names = HashMap::new();
        for phase in [Phase::Handshake, Phase::Status, Phase::Login, Phase::Configuration, Phase::Play] {
            for dir in [Dir::ToClient, Dir::ToServer] {
                let Some(map) = json.get(phase.report_key()).and_then(|p| p.get(dir.report_key())) else {
                    continue;
                };
                let map = map.as_object().ok_or("report: object expected")?;
                for (name, entry) in map {
                    let id = entry
                        .get("protocol_id")
                        .and_then(serde_json::Value::as_i64)
                        .ok_or_else(|| format!("report: protocol_id missing for {name}"))?;
                    names.insert((phase, dir, id as i32), name.clone());
                }
            }
        }
        let mut transitions = HashMap::new();
        for &(t, phase, dir, key) in TRANSITIONS {
            let id = names
                .iter()
                .find(|((p, d, _), n)| *p == phase && *d == dir && n.as_str() == key)
                .map(|((_, _, id), _)| *id)
                .ok_or_else(|| format!("report: transition packet not found ({key})"))?;
            transitions.insert((phase, dir, id), t);
        }
        Ok(Self { names, transitions })
    }

    pub fn name(&self, phase: Phase, dir: Dir, id: i32) -> Option<&str> {
        self.names.get(&(phase, dir, id)).map(String::as_str)
    }

    pub fn transition(&self, phase: Phase, dir: Dir, id: i32) -> Option<Transition> {
        self.transitions.get(&(phase, dir, id)).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }
}
