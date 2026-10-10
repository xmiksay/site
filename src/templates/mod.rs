pub mod context;
pub mod contract;
mod samples;
pub mod smoke;
#[cfg(test)]
mod tests_render;

use std::sync::Arc;

use minijinja::value::Value;
use minijinja::{Environment, UndefinedBehavior};
use parking_lot::RwLock;

use crate::design::{DesignStore, Resolve};

/// MiniJinja templates resolved through a [`DesignStore`].
///
/// Release builds compile every template once at startup and share the
/// resulting environment read-only until a design reload recompiles it
/// ([`Templates::refresh`]). Debug builds rebuild the environment from the
/// design on every render, so editing a template file takes effect on the
/// next request (live reload).
#[derive(Clone)]
pub struct Templates(Source);

#[derive(Clone)]
enum Source {
    /// Release: all templates compiled up front, shared via `Arc`, swapped
    /// on refresh.
    Frozen(Arc<DesignStore>, Arc<RwLock<Arc<Environment<'static>>>>),
    /// Debug: rebuilt from the assets on every render.
    Live(Arc<DesignStore>),
}

impl Templates {
    /// Build the template engine for the given design, picking the strategy
    /// from the build profile.
    pub fn new(design: Arc<DesignStore>) -> Self {
        if cfg!(debug_assertions) {
            Templates(Source::Live(design))
        } else {
            let env = Arc::new(compile_all(&design));
            Templates(Source::Frozen(design, Arc::new(RwLock::new(env))))
        }
    }

    /// An environment ready to render. Frozen returns the shared, precompiled
    /// instance; Live builds a fresh one so on-disk edits are picked up.
    pub fn env(&self) -> Arc<Environment<'static>> {
        match &self.0 {
            Source::Frozen(_, env) => env.read().clone(),
            Source::Live(design) => Arc::new(environment(design.clone())),
        }
    }

    /// Recompile after the design's overrides changed. Live environments
    /// already read the design per render.
    pub fn refresh(&self) {
        if let Source::Frozen(design, env) = &self.0 {
            *env.write() = Arc::new(compile_all(design));
        }
    }
}

/// Compile every available template into the environment up front so release
/// builds never load or compile a template during a request.
fn compile_all(design: &Arc<DesignStore>) -> Environment<'static> {
    let mut env = environment(design.clone());
    let mut count = 0;
    for name in design.template_names() {
        let Some(data) = design.load(&format!("templates/{name}")) else {
            continue;
        };
        match String::from_utf8(data) {
            Ok(src) => match env.add_template_owned(name.clone(), src) {
                Ok(()) => count += 1,
                Err(e) => {
                    tracing::error!(template = %name, error = %e, "template failed to compile")
                }
            },
            Err(e) => tracing::error!(template = %name, error = %e, "template is not valid UTF-8"),
        }
    }
    tracing::info!("templates: compiled {count} template(s) at startup");
    env
}

/// Create an environment with the shared filters and a loader over `design`.
/// The loader is kept even on frozen environments as a safety fallback; in
/// release builds it still resolves entirely from RAM.
pub fn environment(design: Arc<dyn Resolve>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_loader(
        move |name| match design.load(&format!("templates/{name}")) {
            Some(data) => match String::from_utf8(data) {
                Ok(src) => Ok(Some(src)),
                Err(e) => Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("template '{name}' is not valid UTF-8: {e}"),
                )),
            },
            None => Ok(None),
        },
    );
    env.add_filter("timeformat", timeformat);
    env
}

/// The environment the smoke render uses: [`environment`] where any use of
/// an undefined value is an error.
pub fn strict_environment(design: Arc<dyn Resolve>) -> Environment<'static> {
    let mut env = environment(design);
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env
}

fn timeformat(value: Value, format: Option<String>) -> Result<String, minijinja::Error> {
    let s = value.to_string();
    let fmt = format.as_deref().unwrap_or("%d. %m. %Y %H:%M");
    // What `PageView` actually carries (`DateTimeWithTimeZone::to_string`);
    // formatted in its own offset.
    if let Ok(dt) = chrono::DateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f %:z") {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&s) {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(dt.format(fmt).to_string());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return Ok(d.format(fmt).to_string());
    }
    Ok(s)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // pre-existing file layout; not touched by this change
mod tests {
    use super::*;

    fn store() -> Arc<DesignStore> {
        Arc::new(DesignStore::new(None))
    }

    #[test]
    fn compile_all_eagerly_loads_baked_templates() {
        let env = compile_all(&store());
        // Baked `common` templates are present without invoking the loader.
        assert!(env.get_template("base.html").is_ok());
        assert!(env.get_template("404.html").is_ok());
    }

    #[test]
    fn env_renders_a_template() {
        let env = Templates::new(store()).env();
        let layout = context::Layout {
            menu_list: Vec::new(),
            menu_tree: Vec::new(),
            logged_in: false,
        };
        let rendered = env
            .get_template("404.html")
            .unwrap()
            .render(&layout)
            .unwrap();
        assert!(!rendered.is_empty());
    }

    fn format_with(value: &str, fmt: &str) -> String {
        timeformat(Value::from(value), Some(fmt.to_string())).unwrap()
    }

    #[test]
    fn timeformat_formats_page_timestamps_as_serialized() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-03-04T12:30:05.123456+02:00").unwrap();
        let page = crate::entity::page::Model {
            id: 1,
            path: "p".to_string(),
            summary: None,
            markdown: String::new(),
            tag_ids: Vec::new(),
            private: false,
            created_at: at,
            created_by: 1,
            modified_at: at,
            modified_by: 1,
        };
        let view = context::PageView::from(&page);
        assert_eq!(view.modified_at, "2026-03-04 12:30:05.123456 +02:00");
        assert_eq!(
            format_with(&view.modified_at, "%d. %m. %Y %H:%M"),
            "04. 03. 2026 12:30"
        );
    }

    #[test]
    fn timeformat_accepts_other_shapes_and_passes_the_rest_through() {
        let fmt = "%d.%m.%Y";
        assert_eq!(format_with("2026-03-04T12:30:00+00:00", fmt), "04.03.2026");
        assert_eq!(format_with("2026-03-04 12:30:00.5", fmt), "04.03.2026");
        assert_eq!(format_with("2026-03-04T12:30:00", fmt), "04.03.2026");
        assert_eq!(format_with("2026-03-04", fmt), "04.03.2026");
        assert_eq!(format_with("yesterday", fmt), "yesterday");
    }
}
