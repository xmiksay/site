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
        ],
    ),
    ("call", &["web_search", "web_fetch"]),
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
        | "gallery_update" => "path",
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
/// [`expand_capabilities`]. Unmatched calls default to [`Permission::Ask`]
/// (the old `Effect::Prompt` default).
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
    PermissionProfile {
        rules: expand_capabilities(entries, mcp),
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
mod tests {
    use super::*;

    fn rows(rules: &[(&str, Effect, i32, i32)]) -> Vec<tool_permission::Model> {
        rules
            .iter()
            .map(|(name, effect, priority, id)| tool_permission::Model {
                id: *id,
                user_id: 1,
                name: name.to_string(),
                effect: effect.as_str().to_string(),
                priority: *priority,
                created_at: chrono::Utc::now().fixed_offset(),
            })
            .collect()
    }

    /// DB order for `build_profile`'s input: `priority DESC, id DESC` — the
    /// reverse of the old `priority ASC, id ASC` (lower priority number = wins
    /// first), so this test helper mirrors that ordering directly rather than
    /// asking every call site to re-sort.
    fn profile_from(rules: &[(&str, Effect, i32, i32)]) -> PermissionProfile {
        let mut rows = rows(rules);
        rows.sort_by_key(|r| std::cmp::Reverse((r.priority, r.id)));
        build_profile(&rows, &McpCapabilityIndex::new())
    }

    #[test]
    fn bare_read_capability_expands_to_member_tools() {
        let p = profile_from(&[("read", Effect::Allow, 10, 1)]);
        assert_eq!(p.resolve_scoped("page_read", None, None), Permission::Allow);
        assert_eq!(
            p.resolve_scoped("page_search", None, None),
            Permission::Allow
        );
        assert_eq!(p.resolve_scoped("file_list", None, None), Permission::Allow);
        // Not a `read` member — untouched, falls to the Ask default.
        assert_eq!(p.resolve_scoped("page_edit", None, None), Permission::Ask);
    }

    #[test]
    fn bare_write_and_call_capabilities_expand_independently() {
        let p = profile_from(&[
            ("write", Effect::Allow, 20, 1),
            ("call", Effect::Deny, 20, 2),
        ]);
        assert_eq!(p.resolve_scoped("page_edit", None, None), Permission::Allow);
        assert_eq!(
            p.resolve_scoped("tag_create", None, None),
            Permission::Allow
        );
        assert_eq!(p.resolve_scoped("web_search", None, None), Permission::Deny);
        assert_eq!(p.resolve_scoped("web_fetch", None, None), Permission::Deny);
    }

    #[test]
    fn arg_scoped_rule_matches_the_extracted_path() {
        let p = profile_from(&[("page_edit(obsidian/*)", Effect::Allow, 10, 1)]);
        let arg = permission_arg("page_edit", r#"{"path":"obsidian/rust"}"#);
        assert_eq!(
            p.resolve_scoped("page_edit", arg.as_deref(), None),
            Permission::Allow
        );
        let other = permission_arg("page_edit", r#"{"path":"projects/x"}"#);
        assert_eq!(
            p.resolve_scoped("page_edit", other.as_deref(), None),
            Permission::Ask
        );
    }

    #[test]
    fn scoped_capability_rule_fans_out_to_every_member_with_the_pattern() {
        let p = profile_from(&[("write(obsidian/*)", Effect::Deny, 10, 1)]);
        let arg = permission_arg("page_edit", r#"{"path":"obsidian/rust"}"#);
        assert_eq!(
            p.resolve_scoped("page_edit", arg.as_deref(), None),
            Permission::Deny
        );
        // `page_delete` has no `path`-shaped arg extractor collision here —
        // same scoped rule still reaches it since it's a `write` member too.
        let del_arg = permission_arg("page_delete", r#"{"path":"obsidian/rust"}"#);
        assert_eq!(
            p.resolve_scoped("page_delete", del_arg.as_deref(), None),
            Permission::Deny
        );
    }

    #[test]
    fn workdir_scoped_rule_is_stored_but_never_matches_a_site_tool() {
        // #39: `tool{pattern}` parses and is retained like any other rule, but
        // no site tool currently supplies a `workdir` — `resolve` always
        // passes `None`, so this rule can never actually fire yet.
        let p = profile_from(&[("page_edit{/tmp/*}", Effect::Deny, 10, 1)]);
        assert_eq!(p.resolve_scoped("page_edit", None, None), Permission::Ask);
    }

    #[test]
    fn literal_and_wildcard_rules_still_work_unexpanded() {
        let p = profile_from(&[
            ("*", Effect::Deny, 100, 1),
            ("page_read", Effect::Allow, 10, 2),
        ]);
        assert_eq!(p.resolve_scoped("page_read", None, None), Permission::Allow);
        assert_eq!(p.resolve_scoped("page_edit", None, None), Permission::Deny);
    }

    #[test]
    fn priority_ordering_reproduces_first_match_wins_semantics() {
        // Old semantics: lower priority number wins regardless of insertion
        // order. `profile_from` sorts into `priority DESC, id DESC` the same
        // way `resolve`'s DB query does.
        let p = profile_from(&[
            ("page_read", Effect::Deny, 50, 1),
            ("page_read", Effect::Allow, 10, 2),
        ]);
        assert_eq!(p.resolve_scoped("page_read", None, None), Permission::Allow);
    }

    #[test]
    fn mcp_bare_capability_also_covers_an_annotated_mcp_tool() {
        let mut mcp = McpCapabilityIndex::new();
        mcp.insert("read".to_string(), vec!["docs__search".to_string()]);
        let entries = vec![("read".to_string(), Permission::Allow)];
        let rules = expand_capabilities(entries, &mcp);
        let profile = PermissionProfile {
            rules,
            default: Permission::Ask,
        };
        assert_eq!(
            profile.resolve_scoped("docs__search", None, None),
            Permission::Allow
        );
        // A different server's tool, not annotated, is untouched.
        assert_eq!(
            profile.resolve_scoped("docs__unrelated", None, None),
            Permission::Ask
        );
    }

    #[test]
    fn permission_arg_extracts_per_site_tool_shape() {
        assert_eq!(
            permission_arg("page_edit", r#"{"path":"a/b"}"#).as_deref(),
            Some("a/b")
        );
        assert_eq!(
            permission_arg("page_search", r#"{"prefix":"obsidian"}"#).as_deref(),
            Some("obsidian")
        );
        assert_eq!(
            permission_arg("web_search", r#"{"query":"rust"}"#).as_deref(),
            Some("rust")
        );
        assert_eq!(
            permission_arg("web_fetch", r#"{"url":"https://x"}"#).as_deref(),
            Some("https://x")
        );
        // No meaningful scoping argument for this tool.
        assert_eq!(permission_arg("tag_list", r#"{}"#), None);
        // Malformed input.
        assert_eq!(permission_arg("page_edit", "not json"), None);
    }
}
