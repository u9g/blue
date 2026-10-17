//! blue-daemon: connects this machine to the Blue server and runs Claude sessions through
//! claude-bridge.

pub mod agent;
pub mod bridge_proc;
pub mod claude;
pub mod config;
pub mod connection;
pub mod doctor;
pub mod recorder;
pub mod session;
pub mod store;
pub mod translate;
