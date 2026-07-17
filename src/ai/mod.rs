pub mod config;
pub mod handlers;
pub mod tool_permissions;
pub mod ws_bridge;

// Engine (entanglement-core/-runtime/-provider) adapters, wired into
// `state.rs`/`handlers`.
pub mod catalog;
pub mod engine;
pub mod mcp;
pub mod persistence;
pub mod policy;
pub mod projection;
pub mod tools;

pub use config::AiConfig;
