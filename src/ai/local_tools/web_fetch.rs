use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{LocalTool, LocalToolCtx};
use crate::ai::mcp_client::ToolDispatchError;

pub struct WebFetch;

#[derive(Deserialize)]
struct Args {
    url: String,
}

const MAX_BODY_SIZE: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 100_000;

#[async_trait]
impl LocalTool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a web page by URL and return its content converted to Markdown. \
         Use after web_search to read the actual page body. Large pages are truncated."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute URL starting with http:// or https://"
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, _ctx: &LocalToolCtx, args: Value) -> Result<Value, ToolDispatchError> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| ToolDispatchError::Execution(format!("Invalid arguments: {e}")))?;

        if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
            return Err(ToolDispatchError::Execution(
                "url must start with http:// or https://".into(),
            ));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("site-bot/0.1")
            .build()
            .map_err(|e| ToolDispatchError::Execution(e.to_string()))?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| ToolDispatchError::Execution(format!("Fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(ToolDispatchError::Execution(format!(
                "HTTP {}",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ToolDispatchError::Execution(format!("Read body: {e}")))?;

        if bytes.len() > MAX_BODY_SIZE {
            return Err(ToolDispatchError::Execution(format!(
                "Response too large: {} bytes (max {})",
                bytes.len(),
                MAX_BODY_SIZE
            )));
        }

        let html = String::from_utf8_lossy(&bytes).to_string();

        let converter = htmd::HtmlToMarkdown::builder()
            .skip_tags(vec![
                "script", "style", "noscript", "svg", "iframe", "head", "nav", "footer", "form",
            ])
            .build();
        let md = converter
            .convert(&html)
            .unwrap_or_else(|_| strip_html_simple(&html));

        let md = if md.len() > MAX_OUTPUT_CHARS {
            let mut cut = MAX_OUTPUT_CHARS;
            while !md.is_char_boundary(cut) && cut > 0 {
                cut -= 1;
            }
            format!("{}\n\n[... truncated ...]", &md[..cut])
        } else {
            md
        };

        Ok(json!({ "text": format!("# {}\n\n{}", args.url, md) }))
    }
}

fn strip_html_simple(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}
