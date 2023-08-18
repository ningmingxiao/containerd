#![allow(non_snake_case)]

use protobuf::well_known_types::Any;
use std::string::String;

// Mount holds filesystem mount configuration
#[derive(PartialEq, Clone, Default, Debug)]
pub struct Mount {
    pub Type: String,
    pub Source: String,
    pub Target: String,
    pub Options: Vec<String>,
}

impl Mount {
    pub fn new(Type: String, Source: String, Target: String, Options: Vec<String>) -> Self {
        Mount {
            Type,
            Source,
            Target,
            Options,
        }
    }
}

// CreateConfig hold task creation configuration
#[derive(PartialEq, Clone, Default, Debug)]
pub struct CreateConfig {
    pub ID: String,
    pub Bundle: String,
    pub Runtime: String,
    pub Rootfs: Vec<Mount>,
    pub Terminal: bool,
    pub Stdin: String,
    pub Stdout: String,
    pub Stderr: String,
    pub Checkpoint: String,
    pub ParentCheckpoint: String,
    pub Options: Any,
}

// ExecConfig holds exec creation configuration
#[derive(PartialEq, Clone, Default, Debug)]
pub struct ExecConfig {
    pub id: String,
    pub terminal: bool,
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
    pub spec: Any,
}

// CheckpointConfig holds task checkpoint configuration
#[derive(PartialEq, Clone, Default, Debug)]
struct CheckpointConfig {
    Path: String,
    Exit: bool,
    AllowOpenTCP: bool,
    AllowExternalUnixSockets: bool,
    AllowTerminal: bool,
    FileLocks: bool,
    EmptyNamespaces: Vec<String>,
}
