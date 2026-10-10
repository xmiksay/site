//! The render check a design view must pass before it goes live: every
//! template compiles, then the strict smoke render (`templates::smoke`)
//! against the site's real data reports no template error. Publish gates on
//! it; the `design_render_check` tool (#118) runs it over the draft without
//! publishing.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::Serialize;

use super::stored::{DesignError, Files, validate_for_publish};
use crate::storage::Storage;
use crate::templates::smoke::{SmokeError, smoke_render};

/// The outcome of [`render_check`].
#[derive(Debug, Default, Serialize)]
pub struct RenderCheck {
    /// `templates/{name}: {message}` per template that fails to compile. The
    /// smoke render only runs when this is empty.
    pub compile_errors: Vec<String>,
    pub render_errors: Vec<SmokeError>,
    /// How many renders the smoke render performed.
    pub cases: usize,
}

impl RenderCheck {
    pub fn is_ok(&self) -> bool {
        self.compile_errors.is_empty() && self.render_errors.is_empty()
    }

    /// One line per problem, compile and render errors alike.
    pub fn messages(&self) -> Vec<String> {
        let render = self.render_errors.iter().map(smoke_message);
        self.compile_errors.iter().cloned().chain(render).collect()
    }
}

/// Check `view` (a full design: the draft or published files over the baked
/// bundle). `Err` only when the smoke render cannot query the site's data
/// ([`DesignError::RenderCheck`], detail logged).
pub async fn render_check(
    db: &DatabaseConnection,
    storage: &Storage,
    view: &Files,
) -> Result<RenderCheck, DesignError> {
    let compile_errors = validate_for_publish(view);
    if !compile_errors.is_empty() {
        return Ok(RenderCheck {
            compile_errors,
            ..RenderCheck::default()
        });
    }
    let report = smoke_render(db, storage, Arc::new(view.clone()))
        .await
        .map_err(|e| {
            tracing::error!("design render check: smoke render failed: {e:#}");
            DesignError::RenderCheck
        })?;
    Ok(RenderCheck {
        compile_errors,
        render_errors: report.errors,
        cases: report.cases.len(),
    })
}

/// [`render_check`] as a publish gate: any problem is [`DesignError::Invalid`].
pub(super) async fn check_view(
    db: &DatabaseConnection,
    storage: &Storage,
    view: &Files,
) -> Result<(), DesignError> {
    let check = render_check(db, storage, view).await?;
    if check.is_ok() {
        return Ok(());
    }
    Err(DesignError::Invalid(check.messages()))
}

/// `templates/{name}:{line}: {message} (rendering {case})`, in the same
/// `templates/…` path form as the compile errors.
fn smoke_message(e: &SmokeError) -> String {
    let line = e.line.map(|l| format!(":{l}")).unwrap_or_default();
    format!(
        "templates/{}{line}: {} (rendering {})",
        e.template, e.message, e.case
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_errors_read_like_compile_errors() {
        let mut e = SmokeError {
            template: "markdown/fen.html".into(),
            line: Some(2),
            message: "undefined value".into(),
            case: "page `x` (anonymous)".into(),
        };
        assert_eq!(
            smoke_message(&e),
            "templates/markdown/fen.html:2: undefined value (rendering page `x` (anonymous))"
        );
        e.line = None;
        assert!(smoke_message(&e).starts_with("templates/markdown/fen.html: undefined"));
    }

    #[test]
    fn messages_list_compile_then_render_errors() {
        let check = RenderCheck {
            compile_errors: vec!["templates/a.html: syntax error".into()],
            render_errors: vec![SmokeError {
                template: "b.html".into(),
                line: None,
                message: "undefined value".into(),
                case: "404".into(),
            }],
            cases: 3,
        };
        assert!(!check.is_ok());
        assert_eq!(
            check.messages(),
            [
                "templates/a.html: syntax error",
                "templates/b.html: undefined value (rendering 404)"
            ]
        );
        assert!(RenderCheck::default().is_ok());
    }
}
