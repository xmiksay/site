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

#[test]
fn design_tools_default_to_allow_but_user_rules_win() {
    let p = profile_from(&[]);
    for tool in BUILTIN_ALLOW {
        assert_eq!(
            p.resolve_scoped(tool, None, None),
            Permission::Allow,
            "{tool}"
        );
    }
    for spec in crate::design::tools::specs::TOOLS {
        assert!(
            BUILTIN_ALLOW.contains(&spec.name),
            "{} has no default",
            spec.name
        );
    }
    // Everything else still asks.
    assert_eq!(p.resolve_scoped("page_edit", None, None), Permission::Ask);
    // A user's own rule (here a capability) overrides the built-in default.
    let p = profile_from(&[("write", Effect::Prompt, 10, 1)]);
    assert_eq!(
        p.resolve_scoped("design_write", None, None),
        Permission::Ask
    );
    assert_eq!(
        p.resolve_scoped("design_read", None, None),
        Permission::Allow
    );
    let p = profile_from(&[("design_delete", Effect::Deny, 10, 1)]);
    assert_eq!(
        p.resolve_scoped("design_delete", None, None),
        Permission::Deny
    );
}

#[test]
fn design_tools_are_capability_members_scoped_by_path() {
    let p = profile_from(&[("write(assets/*)", Effect::Deny, 10, 1)]);
    let arg = permission_arg("design_write", r#"{"path":"assets/css/x.css"}"#);
    assert_eq!(arg.as_deref(), Some("assets/css/x.css"));
    assert_eq!(
        p.resolve_scoped("design_write", arg.as_deref(), None),
        Permission::Deny
    );
    let other = permission_arg("design_write", r#"{"path":"templates/base.html"}"#);
    assert_eq!(
        p.resolve_scoped("design_write", other.as_deref(), None),
        Permission::Allow
    );
}
