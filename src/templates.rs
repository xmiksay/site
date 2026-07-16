use std::sync::Arc;

use minijinja::Environment;
use minijinja::value::Value;

use crate::design::DesignStore;

/// MiniJinja templates resolved through a [`DesignStore`].
///
/// Release builds compile every template once at startup and share the
/// resulting environment read-only. Debug builds rebuild the environment from
/// the design on every render, so editing a template file takes effect on the
/// next request (live reload).
#[derive(Clone)]
pub struct Templates(Source);

#[derive(Clone)]
enum Source {
    /// Release: all templates compiled up front, shared via `Arc`.
    Frozen(Arc<Environment<'static>>),
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
            Templates(Source::Frozen(Arc::new(compile_all(&design))))
        }
    }

    /// An environment ready to render. Frozen returns the shared, precompiled
    /// instance; Live builds a fresh one so on-disk edits are picked up.
    pub fn env(&self) -> Arc<Environment<'static>> {
        match &self.0 {
            Source::Frozen(env) => env.clone(),
            Source::Live(design) => Arc::new(build_environment(design.clone())),
        }
    }
}

/// Compile every available template into the environment up front so release
/// builds never load or compile a template during a request.
fn compile_all(design: &Arc<DesignStore>) -> Environment<'static> {
    let mut env = build_environment(design.clone());
    let mut count = 0;
    for name in design.template_names() {
        let Some(data) = design.load(&format!("templates/{name}")) else {
            continue;
        };
        match String::from_utf8(data) {
            Ok(src) => match env.add_template_owned(name.clone(), src) {
                Ok(()) => count += 1,
                Err(e) => tracing::error!(template = %name, error = %e, "template failed to compile"),
            },
            Err(e) => tracing::error!(template = %name, error = %e, "template is not valid UTF-8"),
        }
    }
    tracing::info!("templates: compiled {count} template(s) at startup");
    env
}

/// Create an environment with the shared filters and a design-backed loader.
/// The loader is kept even on frozen environments as a safety fallback; in
/// release builds it still resolves entirely from RAM.
fn build_environment(design: Arc<DesignStore>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_loader(move |name| match design.load(&format!("templates/{name}")) {
        Some(data) => match String::from_utf8(data) {
            Ok(src) => Ok(Some(src)),
            Err(e) => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("template '{name}' is not valid UTF-8: {e}"),
            )),
        },
        None => Ok(None),
    });
    env.add_filter("timeformat", timeformat);
    env
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
        let empty: Vec<Value> = Vec::new();
        let rendered = env
            .get_template("404.html")
            .unwrap()
            .render(minijinja::context! {
                logged_in => false,
                menu_list => &empty,
                menu_tree => &empty,
            })
            .unwrap();
        assert!(!rendered.is_empty());
    }
}

fn timeformat(value: Value, format: Option<String>) -> Result<String, minijinja::Error> {
    let s = value.to_string();
    let fmt = format.as_deref().unwrap_or("%d. %m. %Y %H:%M");
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
