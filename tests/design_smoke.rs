//! `templates::smoke::smoke_render` (#117) against a real database: the baked
//! design renders clean under strict undefined handling, and a design with
//! an undefined variable or a syntax error is reported with template + line,
//! also in branches only the contract's example contexts reach.
//!
//! Gated on `DATABASE_URL` (skips when unset). Creates its own throwaway
//! `users`/`pages`/`tags` rows and deletes them before asserting, so a failed
//! assertion leaks nothing.

use std::sync::Arc;

use bytes::Bytes;
use sea_orm::{ActiveModelTrait, Database, DatabaseConnection, EntityTrait, Set};
use site::design::DesignStore;
use site::design::stored::Files;
use site::entity::{page, tag, user};
use site::storage::Storage;
use site::templates::smoke::{SmokeReport, smoke_render};
use tokio::sync::Mutex;

/// Held for each whole test: one test's throwaway rows (a tag, a page) must
/// not appear and vanish under another test's smoke render.
static DB_ROWS: Mutex<()> = Mutex::const_new(());

async fn test_db() -> Option<DatabaseConnection> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        Database::connect(&url)
            .await
            .expect("connect to DATABASE_URL"),
    )
}

/// The baked design with `overrides` (`templates/…` → source) on top.
async fn smoke(db: &DatabaseConnection, overrides: &[(&str, &str)]) -> anyhow::Result<SmokeReport> {
    let overrides: Files = overrides
        .iter()
        .map(|(p, s)| (p.to_string(), Bytes::from(s.to_string())))
        .collect();
    let view = DesignStore::new(None).with_baked(&overrides);
    smoke_render(db, &Storage::db(db.clone()), Arc::new(view)).await
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

/// Smoke-render with a throwaway page using every inline directive in the
/// DB, removed again before the report is returned.
async fn smoke_with_page(db: &DatabaseConnection, overrides: &[(&str, &str)]) -> SmokeReport {
    let id = uuid::Uuid::new_v4();
    let user_id = user::ActiveModel {
        username: Set(format!("design-smoke-{id}")),
        password_hash: Set("unused".to_string()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("insert throwaway user")
    .id;
    let now = chrono::Utc::now().fixed_offset();
    let pg = page::ActiveModel {
        path: Set(format!("design-smoke/{id}")),
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
    .await;

    let report = match &pg {
        Ok(_) => Some(smoke(db, overrides).await),
        Err(_) => None,
    };

    if let Ok(pg) = &pg {
        page::Entity::delete_by_id(pg.id).exec(db).await.unwrap();
    }
    user::Entity::delete_by_id(user_id).exec(db).await.unwrap();
    pg.expect("insert throwaway page");
    report
        .expect("rendered once the page exists")
        .expect("smoke render")
}

#[tokio::test]
async fn baked_design_renders_clean_and_covers_every_template() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _rows = DB_ROWS.lock().await;
    let report = smoke_with_page(&db, &[]).await;
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
}

#[tokio::test]
async fn undefined_variables_are_reported_with_template_and_line() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _rows = DB_ROWS.lock().await;
    let report = smoke_with_page(
        &db,
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
        report
            .errors
            .iter()
            .filter(|e| e.template == "base.html")
            .count(),
        1,
        "{:#?}",
        report.errors
    );
}

#[tokio::test]
async fn syntax_errors_are_reported_with_template_and_line() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _rows = DB_ROWS.lock().await;
    let report = smoke(
        &db,
        &[(
            "templates/404.html",
            "{% extends \"base.html\" %}\n{% block content %}\n{% if %}\n{% endblock %}",
        )],
    )
    .await
    .expect("smoke render");
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

#[tokio::test]
async fn branches_only_examples_reach_are_checked_too() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let _rows = DB_ROWS.lock().await;
    let baked = String::from_utf8(
        DesignStore::new(None)
            .load("templates/page_search.html")
            .expect("baked page_search.html"),
    )
    .unwrap();
    let broken = baked.replacen("{% elif q %}{{ q }}", "{% elif q %}{{ qq }}", 1);
    assert_ne!(broken, baked, "baked page_search.html changed shape");

    // With a tag in the DB the real-data search renders the `tag` branch and
    // never a query; only the example contexts reach `{% elif q %}`.
    let t = tag::ActiveModel {
        name: Set(format!("design-smoke-{}", uuid::Uuid::new_v4())),
        description: Set(None),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("insert throwaway tag");
    let report = smoke(&db, &[("templates/page_search.html", &broken)]).await;
    tag::Entity::delete_by_id(t.id).exec(&db).await.unwrap();
    let report = report.expect("smoke render");

    let err = report
        .errors
        .iter()
        .find(|e| e.template == "page_search.html")
        .unwrap_or_else(|| panic!("page_search error missing: {:#?}", report.errors));
    assert_eq!(err.line, Some(2));
    assert!(err.message.contains("undefined"), "{}", err.message);
    assert!(err.case.starts_with("example context"), "{}", err.case);
}
