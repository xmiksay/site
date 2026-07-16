//! `web_search`/`web_fetch` — port of `local_tools::{web_search, web_fetch}`.
//! Neither needs the calling session at all (no DB scoping), so both keep the
//! plain, non-session `run` override instead of `run_for_session`.

use std::borrow::Cow;

use async_trait::async_trait;
use entanglement_runtime::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

use super::common::parse_args;

pub struct WebSearchTool {
    serper_api_key: Option<String>,
}

impl WebSearchTool {
    pub fn new(serper_api_key: Option<String>) -> Self {
        WebSearchTool { serper_api_key }
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default)]
    num: Option<u32>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("web_search")
    }

    fn description(&self) -> &str {
        "Search the web via Serper. Returns top organic results with title, URL, and snippet. \
         Use to find current information from the public internet."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Prefer 2-5 keywords."
                },
                "num": {
                    "type": "integer",
                    "description": "Number of results (default 5, max 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn run(&self, input: &str) -> anyhow::Result<String> {
        let value = parse_args(input)?;
        let args: SearchArgs =
            serde_json::from_value(value).map_err(|e| anyhow::anyhow!("Invalid arguments: {e}"))?;

        let api_key = self
            .serper_api_key
            .as_deref()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Web search not configured: SERPER_API_KEY not set"))?;

        let num = args.num.unwrap_or(5).min(10);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let body = json!({ "q": args.query, "num": num });

        let resp = client
            .post("https://google.serper.dev/search")
            .header("X-API-KEY", api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Serper request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Serper HTTP {status}: {body}");
        }

        let data: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Serper JSON parse: {e}"))?;

        let mut out = String::new();
        if let Some(kg) = data.get("knowledgeGraph") {
            let title = kg.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let desc = kg.get("description").and_then(|v| v.as_str()).unwrap_or("");
            if !title.is_empty() || !desc.is_empty() {
                out.push_str(&format!("{title}\n{desc}\n\n"));
            }
        }
        if let Some(answer) = data
            .get("answerBox")
            .and_then(|a| a.get("answer").or_else(|| a.get("snippet")))
            .and_then(|v| v.as_str())
        {
            out.push_str(&format!("Answer: {answer}\n\n"));
        }
        if let Some(results) = data.get("organic").and_then(|v| v.as_array()) {
            for (i, r) in results.iter().enumerate() {
                let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let url = r.get("link").and_then(|v| v.as_str()).unwrap_or("");
                let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!("{}. {title}\n{url}\n{snippet}\n\n", i + 1));
            }
        }
        if out.is_empty() {
            out.push_str("No results.");
        }
        Ok(out)
    }
}

pub struct WebFetchTool;

#[derive(Deserialize)]
struct FetchArgs {
    url: String,
}

const MAX_BODY_SIZE: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 100_000;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("web_fetch")
    }

    fn description(&self) -> &str {
        "Fetch a web page by URL and return its content converted to Markdown. Use after \
         web_search to read the actual page body. Large pages are truncated."
    }

    fn schema(&self) -> Value {
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

    async fn run(&self, input: &str) -> anyhow::Result<String> {
        let value = parse_args(input)?;
        let args: FetchArgs =
            serde_json::from_value(value).map_err(|e| anyhow::anyhow!("Invalid arguments: {e}"))?;

        if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
            anyhow::bail!("url must start with http:// or https://");
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("site-bot/0.1")
            .build()?;

        let resp = client
            .get(&args.url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Fetch failed: {e}"))?;

        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Read body: {e}"))?;

        if bytes.len() > MAX_BODY_SIZE {
            anyhow::bail!(
                "Response too large: {} bytes (max {})",
                bytes.len(),
                MAX_BODY_SIZE
            );
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

        Ok(format!("# {}\n\n{}", args.url, md))
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
