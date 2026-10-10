//! The `tool_permissions` rule evaluator `policy.rs` wraps (#39). A user's
//! rows build an `entanglement_core::PermissionProfile` — capability keys
//! (`read`/`write`/`call`) and scoped rule forms (`tool(argpattern)`,
//! `tool{workdirpattern}`) expand into the literal per-tool rules
//! [`PermissionProfile::resolve_scoped`] matches against, then
//! `resolve_scoped` itself (not a bespoke matcher) decides the grade.
//!
//! [`CAPABILITIES`] mirrors the shape of
//! `entanglement_runtime::tool_names::CAPABILITIES`, but over *this site's*
//! built-in tool vocabulary (`page_read`/`page_edit`/…, `src/ai/tools/mod.rs`)
//! rather than the coding-agent's own (`bash`/`edit`/`read`/`grep`/`glob`) —
//! this site's tools don't share those names, so the library's fixed
//! capability table and its `agents::expand_capabilities`/
//! `permission::permission_arg` (both wired to that name set, and the former
//! not even public) don't apply here. [`expand_capabilities`] and
//! [`permission_arg`] below are this site's own analogs of those two,
//! following the same design, over this site's own tools. Every site tool has
//! exactly one capability (no `call`/`rhai`-style tool that spans several), so
//! there's no need for the library's multi-group pre-scan.
//!
//! No site tool currently exposes a working directory (there's no `bash`-like
//! exec tool), so a `tool{pattern}` workdir-scoped rule is accepted and stored
//! like any other but never matches — the resolver always sees `workdir =
//! None` (mirrors how the library itself only supplies a workdir for
//! `bash`/`call`, `None` for everything else).

use std::collections::HashMap;

use entanglement_core::{Permission, PermissionProfile};
use entanglement_runtime::mcp::McpCapabilityIndex;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entity::{tool_permission, user_mcp_server};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Allow,
    Deny,
    Prompt,
}

impl Effect {
    #[allow(clippy::should_implement_trait)] // deliberately infallible, unlike `FromStr::from_str`
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "allow" => Effect::Allow,
            "deny" => Effect::Deny,
            _ => Effect::Prompt,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Allow => "allow",
            Effect::Deny => "deny",
            Effect::Prompt => "prompt",
        }
    }
}

impl From<Effect> for Permission {
    fn from(effect: Effect) -> Self {
        match effect {
            Effect::Allow => Permission::Allow,
            Effect::Deny => Permission::Deny,
            Effect::Prompt => Permission::Ask,
        }
    }
}

/// Capability-level permission keys (#39, mirrors
/// `entanglement_runtime::tool_names::CAPABILITIES`'s shape) over this site's
/// own built-in (non-MCP) tool vocabulary — see the module doc for why the
/// library's own table doesn't apply here. A bare `read: allow` rule grades
/// every read-only tool identically; `write`/`call` likewise.
pub const CAPABILITIES: &[(&str, &[&str])] = &[
    (
        "read",
        &[
            "page_read",
            "page_search",
            "file_list",
            "gallery_list",
            "tag_list",
            "design_list",
            "design_read",
            "design_changes",
            "design_contract",
            "design_render_check",
        ],
    ),
    (
        "write",
        &[
            "page_edit",
            "page_delete",
            "file_create",
            "gallery_create",
            "gallery_update",
            "tag_create",
            "design_write",
            "design_delete",
        ],
    ),
    ("call", &["web_search", "web_fetch"]),
];

/// Tools allowed without approval unless the user's own rules say
/// otherwise: the design tools (#118) only ever touch the shared design
/// draft, which nothing serves until a human publishes it, so asking before
/// every draft edit would only get in the designer's way.
pub const BUILTIN_ALLOW: &[&str] = &[
    "design_list",
    "design_read",
    "design_write",
    "design_delete",
    "design_changes",
    "design_contract",
    "design_render_check",
];

fn capability_members(cap: &str) -> Option<&'static [&'static str]> {
    CAPABILITIES
        .iter()
        .find(|(name, _)| *name == cap)
        .map(|(_, members)| *members)
}

/// A rule key's argument scope, once split from its tool/capability part —
/// a site-local mirror of core's private `RuleScope` (duplicated rather than
/// exposed there, same rationale as the library's own runtime-side mirror).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleScope<'a> {
    None,
    Arg(&'a str),
    Workdir(&'a str),
}

fn split_rule_key(key: &str) -> (&str, RuleScope<'_>) {
    if let Some(open) = key.find('(')
        && key.ends_with(')')
    {
        return (&key[..open], RuleScope::Arg(&key[open + 1..key.len() - 1]));
    }
    if let Some(open) = key.find('{')
        && key.ends_with('}')
    {
        return (
            &key[..open],
            RuleScope::Workdir(&key[open + 1..key.len() - 1]),
        );
    }
    (key, RuleScope::None)
}

/// Expand capability keys (#39) among already-parsed `(key, permission)`
/// entries (file order — here, DB row order, `priority DESC, id DESC` so the
/// **last** entry is this profile's highest-precedence rule, matching
/// `PermissionProfile::resolve_scoped`'s last-match-wins) into the literal
/// per-tool rules it actually matches against:
///
/// - a non-capability key (a literal tool name, `*`, or an already-scoped
///   literal like `page_edit(obsidian/*)`) is pushed verbatim;
/// - a bare capability key (`read`/`write`/`call`) pushes its member tools
///   plus any MCP tool `mcp` annotates with that capability (#39, ADR-0117);
/// - a scoped capability key (`read(obsidian/*)`/`write{...}`) pushes
///   `member(pattern)`/`member{pattern}` for each *built-in* member only — an
///   MCP tool has no known argument shape to scope against, mirroring the
///   library's own `arg_scoped_capability_members` restriction.
fn expand_capabilities(
    entries: Vec<(String, Permission)>,
    mcp: &McpCapabilityIndex,
) -> Vec<(String, Permission)> {
    let mut rules = Vec::with_capacity(entries.len());
    for (key, perm) in entries {
        let (name, scope) = split_rule_key(&key);
        let name = name.to_string();
        match scope {
            RuleScope::None => match capability_members(&name) {
                Some(members) => {
                    rules.extend(members.iter().map(|m| (m.to_string(), perm)));
                    rules.extend(
                        mcp.get(&name)
                            .into_iter()
                            .flatten()
                            .map(|m| (m.clone(), perm)),
                    );
                }
                None => rules.push((key, perm)),
            },
            RuleScope::Arg(pattern) => match capability_members(&name) {
                Some(members) => {
                    rules.extend(members.iter().map(|m| (format!("{m}({pattern})"), perm)));
                }
                None => rules.push((key, perm)),
            },
            RuleScope::Workdir(pattern) => match capability_members(&name) {
                Some(members) => {
                    rules.extend(members.iter().map(|m| (format!("{m}{{{pattern}}}"), perm)));
                }
                None => rules.push((key, perm)),
            },
        }
    }
    rules
}

/// The tool-specific argument an argument-scoped rule (`tool(pattern)`)
/// matches against, for this site's own tools — the module doc explains why
/// this can't be `entanglement_runtime::permission::permission_arg`. `None`
/// for a tool with no single meaningful scoping argument, or malformed input.
pub fn permission_arg(tool: &str, input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let field = match tool {
        "page_read" | "page_edit" | "page_delete" | "file_create" | "gallery_create"
        | "gallery_update" | "design_read" | "design_write" | "design_delete" => "path",
        "page_search" => "prefix",
        "web_search" => "query",
        "web_fetch" => "url",
        _ => return None,
    };
    value.get(field)?.as_str().map(String::from)
}

/// Build the config-side MCP capability fan-out index (#39, ADR-0117) for
/// `user_id`: capability name → every `"{server}__{tool}"` identity a
/// server's `capabilities` annotation maps to it (`ai::mcp::SiteMcp`'s own
/// naming, not `entanglement_runtime::mcp`'s `mcp__`-prefixed one — see the
/// module doc). Doesn't require a server to actually be connected; an
/// annotation naming a tool the server doesn't (yet, or ever) expose is
/// simply inert, and an unknown capability value is ignored here (rejected
/// up front instead, at CRUD time — `ai::handlers::mcp_servers`).
pub async fn mcp_capability_index(
    db: &DatabaseConnection,
    user_id: i32,
) -> anyhow::Result<McpCapabilityIndex> {
    let servers = user_mcp_server::Entity::find()
        .filter(user_mcp_server::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    let mut index: McpCapabilityIndex = HashMap::new();
    for server in &servers {
        let Some(caps) = server.capabilities.as_object() else {
            continue;
        };
        for (tool, capability) in caps {
            let Some(capability) = capability.as_str() else {
                continue;
            };
            if capability_members(capability).is_none() {
                continue;
            }
            index
                .entry(capability.to_string())
                .or_default()
                .push(format!("{}__{}", server.name, tool));
        }
    }
    for members in index.values_mut() {
        members.sort();
    }
    Ok(index)
}

/// Build a user's effective [`PermissionProfile`] from their `tool_permissions`
/// rows, ordered `priority DESC, id DESC` (ascending precedence, so
/// `resolve_scoped`'s last-match-wins reproduces the old `priority ASC, id
/// ASC` first-match-wins semantics) and expanded through
/// [`expand_capabilities`], over the [`BUILTIN_ALLOW`] defaults. Unmatched
/// calls default to [`Permission::Ask`] (the old `Effect::Prompt` default).
pub fn build_profile(
    rows: &[tool_permission::Model],
    mcp: &McpCapabilityIndex,
) -> PermissionProfile {
    let entries = rows
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                Permission::from(Effect::from_str(&r.effect)),
            )
        })
        .collect();
    // First = lowest precedence (last match wins): any user rule beats these.
    let mut rules: Vec<(String, Permission)> = BUILTIN_ALLOW
        .iter()
        .map(|tool| (tool.to_string(), Permission::Allow))
        .collect();
    rules.extend(expand_capabilities(entries, mcp));
    PermissionProfile {
        rules,
        default: Permission::Ask,
    }
}

/// Resolve the effective permission for a tool call: load `user_id`'s rules
/// and MCP capability index, build the profile, and grade `tool_name` against
/// it — `arg`/`workdir` scope an argument-/workdir-scoped rule to the actual
/// call (#39, `PermissionProfile::resolve_scoped`).
pub async fn resolve(
    db: &DatabaseConnection,
    user_id: i32,
    tool_name: &str,
    arg: Option<&str>,
    workdir: Option<&str>,
) -> anyhow::Result<Permission> {
    let rows = tool_permission::Entity::find()
        .filter(tool_permission::Column::UserId.eq(user_id))
        .order_by_desc(tool_permission::Column::Priority)
        .order_by_desc(tool_permission::Column::Id)
        .all(db)
        .await?;
    let mcp = mcp_capability_index(db, user_id).await?;
    let profile = build_profile(&rows, &mcp);
    Ok(profile.resolve_scoped(tool_name, arg, workdir))
}

#[cfg(test)]
mod tests;
