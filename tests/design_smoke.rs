//! `templates::smoke::smoke_render` (#117) against a real database: the baked
//! design renders clean under strict undefined handling, and a design with
//! an undefined variable or a syntax error is reported with template + line.
//!
//! Gated on `DATABASE_URL` (skips when unset). Creates its own throwaway
//! `users`/`pages` rows and deletes them when done.

use std::sync::Arc;

use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::design::DesignStore;
use site::entity::{page, user};
use site::storage::Storage;
use site::templates::smoke::{SmokeReport, smoke_render};

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

/// The baked design with `overrides` (`templates/…` → source) on top.
async fn smoke(db: &DatabaseConnection, overrides: &[(&str, &str)]) -> SmokeReport {
    let design = Arc::new(DesignStore::new(None));
    let overrides: Vec<(String, String)> = overrides
        .iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect();
    let load = move |path: &str| match overrides.iter().find(|(p, _)| p == path) {
        Some((_, src)) => Some(src.clone().into_bytes()),
        None => design.load(path),
    };
    smoke_render(db, &Storage::db(db.clone()), load)
        .await
        .expect("smoke render")
}

const DIRECTIVES_MD: &str = r#"# Smoke

<fen size="large">rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1</fen>

<pgn move="2">1. e4 e5 2. Nf3 Nc6</pgn>

```mermaid
graph TD
  A --> B
```

<json query=".rows[]" type="table">{"rows": [{"a": 1, "b": "x"}]}</json>
"#;

async fn with_page<F: AsyncFnOnce(&DatabaseConnection, &str)>(db: &DatabaseConnection, f: F) {
    let tag = uuid::Uuid::new_v4();
    let user_id = user::ActiveModel {
        username: Set(format!("design-smoke-{tag}")),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user")
    .id;
    let now = chrono::Utc::now().fixed_offset();
    let path = format!("design-smoke/{tag}");
    let pg = page::ActiveModel {
        path: Set(path.clone()),
        summary: Set(Some("smoke".to_string())),
        markdown: Set(DIRECTIVES_MD.to_string()),
        tag_ids: Set(vec![]),
        private: Set(true),
        created_at: Set(now),
        created_by: Set(user_id),
        modified_at: Set(now),
        modified_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway page");

    f(db, &path).await;

    page::Entity::delete_by_id(pg.id).exec(db).await.unwrap();
    user::Entity::delete_by_id(user_id).exec(db).await.unwrap();
}

#[tokio::test]
async fn baked_design_renders_clean_and_covers_every_template() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    with_page(&db, async |db, _path| {
        let report = smoke(db, &[]).await;
        assert!(report.is_ok(), "{:#?}", report.errors);
        for spec in site::templates::contract::TEMPLATES {
            assert!(
                report
                    .cases
                    .iter()
                    .any(|c| c.starts_with(&format!("{}: ", spec.name))),
                "{} never rendered: {:#?}",
                spec.name,
                report.cases
            );
        }
    })
    .await;
}

#[tokio::test]
async fn undefined_variables_are_reported_with_template_and_line() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    with_page(&db, async |db, _path| {
        let report = smoke(
            db,
            &[
                (
                    "templates/markdown/fen.html",
                    "<div>{{ fen }}</div>\n<p>{{ no_such_var }}</p>",
                ),
                (
                    "templates/base.html",
                    "<title>{% block title %}{% endblock %}</title>\n{{ site.name }}\n{% block content %}{% endblock %}",
                ),
            ],
        )
        .await;
        let fen = report
            .errors
            .iter()
            .find(|e| e.template == "markdown/fen.html")
            .unwrap_or_else(|| panic!("fen error missing: {:#?}", report.errors));
        assert_eq!(fen.line, Some(2));
        assert!(fen.message.contains("undefined"), "{}", fen.message);

        let base = report
            .errors
            .iter()
            .find(|e| e.template == "base.html")
            .unwrap_or_else(|| panic!("base error missing: {:#?}", report.errors));
        assert_eq!(base.line, Some(2));
        // Reported once although every page template extends it.
        assert_eq!(
            report.errors.iter().filter(|e| e.template == "base.html").count(),
            1,
            "{:#?}",
            report.errors
        );
    })
    .await;
}

#[tokio::test]
async fn syntax_errors_are_reported_with_template_and_line() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let report = smoke(
        &db,
        &[(
            "templates/404.html",
            "{% extends \"base.html\" %}\n{% block content %}\n{% if %}\n{% endblock %}",
        )],
    )
    .await;
    let err = report
        .errors
        .iter()
        .find(|e| e.template == "404.html")
        .unwrap_or_else(|| panic!("404 error missing: {:#?}", report.errors));
    assert_eq!(err.line, Some(3));
    assert!(err.message.contains("syntax error"), "{}", err.message);
    assert!(
        report.errors.iter().all(|e| e.template == "404.html"),
        "only 404.html is broken: {:#?}",
        report.errors
    );
}
