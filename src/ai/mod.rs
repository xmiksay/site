pub mod config;
pub mod handlers;
pub mod llm;
pub mod local_tools;
pub mod mcp_client;
pub mod tool_permissions;
pub mod tool_registry;

// New engine (entanglement-core/-runtime/-provider) adapters — Phase 1 of the
// engine swap (issue #15), wired into `state.rs`/`handlers` as of the
// follow-up phase. `llm`, `local_tools`, `mcp_client`, and `tool_registry`
// above are the *old* engine's modules: no longer reachable from `AppState`
// (nothing constructs a `ProviderRegistry`/`ToolRegistry`/`UserMcpManager`
// anymore), but kept compiling standalone per issue #15 — Phase 4 of the
// larger migration deletes them. (`loop_driver`, the old turn-driving logic,
// was deleted outright rather than gutted: it referenced the now-removed
// `assistant_message` entity, and nothing calls it once `AppState` dropped
// the fields its signature required, so a half-gutted stub would have been
// dead weight rather than honestly-compiling code.)
pub mod catalog;
pub mod engine;
pub mod mcp;
pub mod persistence;
pub mod policy;
pub mod projection;
pub mod tools;

pub use config::AiConfig;
