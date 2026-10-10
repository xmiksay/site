//! The engine's agent-profile roster (#17): the built-in root profile plus
//! the spawnable sub-agents, `researcher`, `page-writer` and (#118)
//! `designer`. Split out of `engine.rs` to keep that file under the
//! project's 400-line cap.

use std::sync::LazyLock;

use entanglement_core::{AgentMode, AgentProfile, Permission, PermissionProfile, ProfileRegistry};

use crate::design::tools::specs as design_specs;
use crate::markdown::MARKDOWN_EXTENSIONS_DOC;
use crate::templates::contract;

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

/// Sub-agent profile name: edits the shared design draft (#118) — never
/// publishes. Same leaf restriction as [`RESEARCHER_PROFILE`].
pub const DESIGNER_PROFILE: &str = "designer";

/// Every profile name a session may directly switch to via `InMsg::SetAgent`
/// (#42) — the root plus the spawnable sub-agents. `entanglement_core`
/// itself imposes no reachability gate on a direct `SetAgent` (only spawn
/// targets are mode-checked), so this site enforces its own known-name
/// allowlist at the API boundary instead of forwarding an arbitrary string to
/// the engine.
pub const SWITCHABLE_PROFILES: &[&str] = &[
    BUILD_PROFILE,
    RESEARCHER_PROFILE,
    PAGE_WRITER_PROFILE,
    DESIGNER_PROFILE,
];

const RESEARCHER_TOOLS: &[&str] = &[
    "web_search",
    "web_fetch",
    "page_read",
    "page_search",
    "tag_list",
    "file_list",
    "gallery_list",
];

const PAGE_WRITER_TOOLS: &[&str] = &[
    "page_read",
    "page_search",
    "page_edit",
    "tag_create",
    "file_create",
    "gallery_list",
    "gallery_create",
    "gallery_update",
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

/// Appended for a session running under [`DESIGNER_PROFILE`]. Carries the
/// template contract itself, without its example contexts (~10 KB instead
/// of ~16 KB): a designer edits templates in nearly every task, so the
/// variable tables earn their place, and injecting them saves a
/// `design_contract` round trip; the examples and the JSON Schema stay one
/// call away. Generated from the same source as `docs/design-contract.md`,
/// so it cannot drift.
static DESIGNER_PROMPT_SUFFIX: LazyLock<String> = LazyLock::new(|| {
    format!(
        "\n\n---\n\nYou are running as the `designer` sub-agent: you change the public site's \
         design (MiniJinja templates under `templates/`, CSS/JS/fonts/images under `assets/`, \
         export layouts under `mdcast/`) by editing the shared design **draft** with the \
         `design_*` tools. The live site does not change until a human publishes the draft; you \
         cannot publish.\n\n\
         Workflow: `design_list`/`design_read` the files you change (`source: \"baked\"` shows \
         the shipped default) → `design_write` (text as `data`, binary as `data_base64`) or \
         `design_delete` (reverts a file to its baked default) → `design_render_check`, and fix \
         every compile or render error it reports until it answers `ok: true` → \
         `design_changes`. Finish by summarizing the changes and telling the human to preview \
         the draft and publish it from the admin Design page.\n\n\
         Templates may only use the variables the contract below lists: the render check \
         treats anything else as an error. `design_contract` returns it with example contexts \
         (`schema: true` for JSON Schema).\n\n\
         Page bodies arrive pre-rendered; the markdown directives below render through the \
         `templates/markdown/*.html` partials:\n\n{MARKDOWN_EXTENSIONS_DOC}\n\n{}",
        contract::compact_markdown()
    )
});

/// The profile-specific part of the system prompt, appended to the site
/// prompt (`engine.rs`'s `system_prompt_resolver`); empty for the root.
pub(super) fn prompt_suffix(profile: &str) -> &'static str {
    match profile {
        RESEARCHER_PROFILE => RESEARCHER_PROMPT_SUFFIX,
        PAGE_WRITER_PROFILE => PAGE_WRITER_PROMPT_SUFFIX,
        DESIGNER_PROFILE => DESIGNER_PROMPT_SUFFIX.as_str(),
        _ => "",
    }
}

/// The engine's agent-profile roster (#17): the built-in root profile (every
/// session starts under it) plus the spawnable sub-agents. The root
/// profile's `spawnable_agents` allowlist is narrowed to exactly these —
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
            DESIGNER_PROFILE.to_string(),
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
    // Design tools only: nothing that ingests outside content (web, pages,
    // files, MCP servers), because draft writes are approval-free under this
    // profile (`tool_permissions::DESIGN_WRITE_TOOLS`) and the draft runs on
    // the site's origin in an admin's preview.
    let designer_tools: Vec<&str> = design_specs::TOOLS.iter().map(|spec| spec.name).collect();
    registry.insert(sub_agent_profile(
        DESIGNER_PROFILE,
        "Design sub-agent — edits the shared design draft (templates, CSS, JS, assets) and \
         validates it with the strict render check; never publishes (a human does, in the admin).",
        &designer_tools,
    ));
    registry
}

/// Build one of the leaf sub-agent profiles: `Subagent` mode (reachable
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
        // `None` inherits the executor's `SandboxConfig` base (engine.rs wires
        // `SandboxConfig::none()`), which is this site's unconfined status quo —
        // the sandbox only governs `bash`/`call`, neither of which is registered.
        sandbox: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_profile_may_only_spawn_the_sub_agents() {
        let profiles = build_profiles();
        let root = profiles.get("build").expect("build profile present");
        let allowed: Option<Vec<&str>> = root
            .spawnable_agents
            .as_ref()
            .map(|v| v.iter().map(String::as_str).collect());
        assert_eq!(
            allowed,
            Some(vec![
                RESEARCHER_PROFILE,
                PAGE_WRITER_PROFILE,
                DESIGNER_PROFILE
            ])
        );
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
        assert!(researcher.advertises_tool("page_read"));
        assert!(researcher.advertises_tool("web_search"));
        assert!(!researcher.advertises_tool("page_edit"));
        assert!(!researcher.advertises_tool("file_create"));

        assert!(page_writer.spawnable_as_subagent());
        assert!(
            !page_writer.may_spawn(),
            "page-writer must not itself spawn"
        );
        assert!(page_writer.advertises_tool("page_edit"));
        assert!(page_writer.advertises_tool("gallery_create"));
        assert!(!page_writer.advertises_tool("web_search"));
        assert!(!page_writer.advertises_tool("page_delete"));
    }

    #[test]
    fn designer_is_a_leaf_with_every_design_tool_and_no_publish() {
        let profiles = build_profiles();
        let designer = profiles.get(DESIGNER_PROFILE).expect("designer");
        assert!(designer.spawnable_as_subagent());
        assert!(!designer.may_spawn());
        for spec in design_specs::TOOLS {
            assert!(designer.advertises_tool(spec.name), "{}", spec.name);
        }
        // No tool that ingests outside content: its writes need no approval.
        for tool in [
            "web_fetch",
            "web_search",
            "page_read",
            "page_search",
            "file_read",
            "file_list",
            "srv__fetch",
        ] {
            assert!(!designer.advertises_tool(tool), "{tool}");
        }
        assert!(SWITCHABLE_PROFILES.contains(&DESIGNER_PROFILE));
    }

    #[test]
    fn designer_prompt_carries_contract_directives_and_workflow() {
        let prompt = prompt_suffix(DESIGNER_PROFILE);
        assert!(prompt.contains("`designer` sub-agent"));
        assert!(prompt.contains(MARKDOWN_EXTENSIONS_DOC));
        assert!(prompt.contains(&contract::compact_markdown()));
        assert!(prompt.contains("design_render_check"));
        assert!(prompt.contains("publish it from the admin"));
        assert_eq!(prompt_suffix(BUILD_PROFILE), "");
        assert_eq!(prompt_suffix(RESEARCHER_PROFILE), RESEARCHER_PROMPT_SUFFIX);
    }
}
