//! The engine's agent-profile roster (#17): the built-in root profile plus
//! the two spawnable sub-agents, `researcher` and `page-writer`. Split out of
//! `engine.rs` to keep that file under the project's 400-line cap.

use entanglement_core::{AgentMode, AgentProfile, Permission, PermissionProfile, ProfileRegistry};

/// The engine's built-in root profile name (`entanglement_core::ProfileRegistry
/// ::new`'s own constant) — every session starts under it. Re-exported here
/// (rather than only living as a string literal) so `handlers/sessions` can
/// validate/default a `SetAgent` target against the full roster in one place
/// (#42).
pub const BUILD_PROFILE: &str = "build";

/// Sub-agent profile name: read-only research support (#17). Spawnable from
/// the root profile only — `can_spawn: Some(false)` keeps it a leaf so a
/// research task can't itself fan out further sub-agents.
pub const RESEARCHER_PROFILE: &str = "researcher";

/// Sub-agent profile name: drafts/edits a single page (#17). Same leaf
/// restriction as [`RESEARCHER_PROFILE`].
pub const PAGE_WRITER_PROFILE: &str = "page-writer";

/// Every profile name a session may directly switch to via `InMsg::SetAgent`
/// (#42) — the root plus the two spawnable sub-agents. `entanglement_core`
/// itself imposes no reachability gate on a direct `SetAgent` (only spawn
/// targets are mode-checked), so this site enforces its own known-name
/// allowlist at the API boundary instead of forwarding an arbitrary string to
/// the engine.
pub const SWITCHABLE_PROFILES: &[&str] = &[BUILD_PROFILE, RESEARCHER_PROFILE, PAGE_WRITER_PROFILE];

const RESEARCHER_TOOLS: &[&str] = &[
    "web_search",
    "web_fetch",
    "read_page",
    "search_pages",
    "list_tags",
    "list_files",
    "list_galleries",
];

const PAGE_WRITER_TOOLS: &[&str] = &[
    "read_page",
    "search_pages",
    "edit_page",
    "create_tag",
    "create_file",
    "list_galleries",
    "create_gallery",
    "update_gallery",
];

/// Appended to the site system prompt (`engine.rs`'s `system_prompt_resolver`)
/// only for a session running under [`RESEARCHER_PROFILE`] — the model
/// otherwise gets the exact same generic prompt regardless of profile.
pub(super) const RESEARCHER_PROMPT_SUFFIX: &str = "\n\n---\n\nYou are running as the `researcher` \
    sub-agent, delegated a single research task by the primary assistant. You are read-only: \
    search and read existing pages/files and the web, then report your findings in prose. You \
    have no edit/create/delete tools — do not attempt to use one.";

/// Appended to the site system prompt only for a session running under
/// [`PAGE_WRITER_PROFILE`].
pub(super) const PAGE_WRITER_PROMPT_SUFFIX: &str = "\n\n---\n\nYou are running as the `page-writer` \
    sub-agent, delegated a single page-drafting task by the primary assistant. Search first to \
    avoid duplicating an existing page, then create or edit exactly the page you were asked for \
    (private by default). Report the page's path back when done.";

/// The engine's agent-profile roster (#17): the built-in root profile (every
/// session starts under it) plus the two spawnable sub-agents. The root
/// profile's `spawnable_agents` allowlist is narrowed to exactly these two —
/// combined with `entanglement_runtime`'s ancestor privilege clamp (a child's
/// effective tool mask/permission grade is the least-privileged fold across
/// its own profile and every ancestor's), a sub-agent can never reach a tool
/// or permission grade the user's root session itself doesn't already have.
pub(super) fn build_profiles() -> ProfileRegistry {
    let mut registry = ProfileRegistry::new(); // inserts the built-in "build" root profile
    if let Some(mut root) = registry.get("build").cloned() {
        root.spawnable_agents = Some(vec![
            RESEARCHER_PROFILE.to_string(),
            PAGE_WRITER_PROFILE.to_string(),
        ]);
        registry.insert(root);
    }
    registry.insert(sub_agent_profile(
        RESEARCHER_PROFILE,
        "Read-only research sub-agent — searches and reads existing pages/files and the web; \
         cannot create, edit, or delete anything.",
        RESEARCHER_TOOLS,
    ));
    registry.insert(sub_agent_profile(
        PAGE_WRITER_PROFILE,
        "Page-drafting sub-agent — creates or edits a single page (private by default) plus its \
         supporting tags/files/galleries.",
        PAGE_WRITER_TOOLS,
    ));
    registry
}

/// Build one of the two leaf sub-agent profiles: `Subagent` mode (reachable
/// only via spawn, never a primary entry agent), restricted to `tools` (#116's
/// physical tool mask — anything else is neither advertised nor accepted),
/// and `can_spawn: Some(false)` so it cannot itself spawn further (depth stays
/// at 1 regardless of `entanglement_runtime`'s own `MAX_SPAWN_DEPTH`).
fn sub_agent_profile(name: &str, description: &str, tools: &[&str]) -> AgentProfile {
    AgentProfile {
        name: name.to_string(),
        description: description.to_string(),
        mode: AgentMode::Subagent,
        // Dead in practice: `system_prompt_resolver` (engine.rs) always
        // returns `Some`, so core never falls back to this field. Kept
        // non-empty anyway so a profile dump/log is self-explanatory without
        // cross referencing the resolver.
        system_prompt: description.to_string(),
        model: None,
        provider: None,
        // The real Allow/Ask/Deny grade comes from `SitePolicy` (the DB-backed
        // `PermissionResolver`), not this field — leaving it allow-all so the
        // tool mask below is this profile's only *structural* restriction.
        permission: PermissionProfile::new(Permission::Allow),
        tools: Some(tools.iter().map(|s| s.to_string()).collect()),
        disallowed_tools: Vec::new(),
        can_spawn: Some(false),
        spawnable_agents: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_profile_may_only_spawn_the_two_sub_agents() {
        let profiles = build_profiles();
        let root = profiles.get("build").expect("build profile present");
        let allowed: Option<Vec<&str>> = root
            .spawnable_agents
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(allowed, Some(vec![RESEARCHER_PROFILE, PAGE_WRITER_PROFILE]));
        assert!(root.may_spawn());
        assert!(root.spawn_target_allowed(RESEARCHER_PROFILE));
        assert!(root.spawn_target_allowed(PAGE_WRITER_PROFILE));
        assert!(!root.spawn_target_allowed("some-future-profile"));
    }

    #[test]
    fn sub_agent_profiles_are_leaves_restricted_to_their_own_tools() {
        let profiles = build_profiles();
        let researcher = profiles.get(RESEARCHER_PROFILE).expect("researcher");
        let page_writer = profiles.get(PAGE_WRITER_PROFILE).expect("page-writer");

        assert!(researcher.spawnable_as_subagent());
        assert!(!researcher.may_spawn(), "researcher must not itself spawn");
        assert!(researcher.advertises_tool("read_page"));
        assert!(researcher.advertises_tool("web_search"));
        assert!(!researcher.advertises_tool("edit_page"));
        assert!(!researcher.advertises_tool("create_file"));

        assert!(page_writer.spawnable_as_subagent());
        assert!(
            !page_writer.may_spawn(),
            "page-writer must not itself spawn"
        );
        assert!(page_writer.advertises_tool("edit_page"));
        assert!(page_writer.advertises_tool("create_gallery"));
        assert!(!page_writer.advertises_tool("web_search"));
        assert!(!page_writer.advertises_tool("delete_page"));
    }
}
