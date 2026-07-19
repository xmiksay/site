//! Static MCP server content: the `initialize` instructions text (default,
//! overridable by a `CLAUDE` page — see `super::handle_initialize`) and the
//! `tools/list` schema catalogue. Kept separate from `mod.rs` because it's
//! almost entirely inert string/JSON data rather than endpoint logic.

use serde_json::{Value, json};

use super::rpc::JsonRpcResponse;

pub(super) fn server_instructions() -> String {
    format!(
        "{SERVER_INSTRUCTIONS_HEADER}\n{}\n",
        crate::markdown::MARKDOWN_EXTENSIONS_DOC
    )
}

const SERVER_INSTRUCTIONS_HEADER: &str = "\
# Site — MCP Integration

Server-rendered site. Pages are stored in PostgreSQL and served at their `path` \
(e.g. path `notes/example` → URL `/notes/example`).

Pages, tags, files and galleries can all be managed here as MCP tools. To \
override these instructions for your installation, create a page with path \
`CLAUDE` and its markdown will be served instead.

## Pages

- **path**: unique URL slug. Hierarchical paths use `/` (e.g. `section/sub/page`).
- **markdown**: content in Markdown with custom extensions (see below).
- **summary**: short description for listings.
- **tags**: assigned by name via `edit_page`; names that don't exist yet are \
  skipped (create them first with `create_tag` to attach them).
- **private**: private pages are only visible to logged-in users. \
  New pages created via MCP default to private.
- **revisions**: every markdown change stores a diff automatically.

## Markdown extensions

";

pub(super) fn handle_tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "tools": [
                // ----- Pages -----
                {
                    "name": "read_page",
                    "description": "Read a page by its path. Returns title (path), summary, tags, and full markdown content.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "path": { "type": "string", "description": "The page path (e.g. 'section/sub/page')" } },
                        "required": ["path"]
                    }
                },
                {
                    "name": "edit_page",
                    "description": "Create or update a page by its path. Creates the page if it doesn't exist. A revision diff is stored automatically when markdown changes.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "The page path to edit" },
                            "markdown": { "type": "string", "description": "New markdown content (optional)" },
                            "summary": { "type": "string", "description": "New summary (optional)" },
                            "tag_names": { "type": "array", "items": { "type": "string" }, "description": "Tag names to assign (optional, replaces existing tags)" },
                            "private": { "type": "boolean", "description": "Visibility flag (optional, defaults to true on create)" }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "search_pages",
                    "description": "Search pages by path prefix, tag name, and/or fulltext query (q). Path and tag matches rank above markdown content matches. Returns path, summary for each match, plus total count and has_more flag for pagination.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "prefix": { "type": "string", "description": "Path prefix to filter by (case-insensitive). If omitted, returns all pages." },
                            "tag": { "type": "string", "description": "Optional tag name — only returns pages with this tag" },
                            "q": { "type": "string", "description": "Optional fulltext query (accent-insensitive); ranks path and tag matches above markdown content" },
                            "limit": { "type": "integer", "description": "Max results to return (default 20, max 100)" },
                            "offset": { "type": "integer", "description": "Number of results to skip for pagination (default 0)" }
                        }
                    }
                },
                {
                    "name": "delete_page",
                    "description": "Delete a page by its path.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                },

                // ----- Tags -----
                {
                    "name": "list_tags",
                    "description": "List all available tags. Returns tag name and description.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "read_tag",
                    "description": "Read a single tag by name.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }
                },
                {
                    "name": "create_tag",
                    "description": "Create a new tag.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "update_tag",
                    "description": "Update an existing tag's name and/or description (look up by current name).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Current name (lookup key)" },
                            "new_name": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["name"]
                    }
                },
                {
                    "name": "delete_tag",
                    "description": "Delete a tag by name.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    }
                },

                // ----- Files -----
                {
                    "name": "list_files",
                    "description": "List uploaded files. Optionally filter by mimetype prefix (e.g. 'image/').",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "mime_prefix": { "type": "string" }
                        }
                    }
                },
                {
                    "name": "create_file",
                    "description": "Upload a file at the given path. Provide either `data_base64` (binary, e.g. images) or `data` (raw text, e.g. PGN/FEN/SVG). The display title is derived from the basename of the path. Returns the new file id plus an `embed` hint for the matching markdown directive: `<image id=\"ID\">` for images, `<pgn id=\"ID\">` for `.pgn`, `<mermaid id=\"ID\">` for `.mmd`, `<fen id=\"ID\">` for `.fen`, `<json id=\"ID\" query=\"...\">` for `.json`, `<file id=\"ID\">` otherwise (or `<gallery id=\"ID\">` to group several files). Generates a thumbnail automatically for images.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Filename or path used as the file's identifier (must be unique)." },
                            "description": { "type": "string" },
                            "mimetype": { "type": "string", "description": "e.g. image/png. Inferred from the path's extension when omitted." },
                            "data_base64": { "type": "string", "description": "Base64-encoded binary contents." },
                            "data": { "type": "string", "description": "Raw text contents (alternative to data_base64)." }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "read_file",
                    "description": "Read file metadata by ID (does not return binary contents).",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },
                {
                    "name": "update_file",
                    "description": "Update file metadata (path, description). The display title is always derived from the path basename.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "path": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["id", "path"]
                    }
                },
                {
                    "name": "delete_file",
                    "description": "Delete a file by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },

                // ----- Galleries -----
                {
                    "name": "list_galleries",
                    "description": "List all galleries.",
                    "inputSchema": { "type": "object", "properties": {} }
                },
                {
                    "name": "read_gallery",
                    "description": "Read a gallery by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                },
                {
                    "name": "create_gallery",
                    "description": "Create a gallery from a list of file IDs. `path` is the unique URL slug (e.g. `holiday-2024`).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "file_ids": { "type": "array", "items": { "type": "integer" } }
                        },
                        "required": ["path", "title"]
                    }
                },
                {
                    "name": "update_gallery",
                    "description": "Update a gallery (replaces all fields).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "path": { "type": "string" },
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "file_ids": { "type": "array", "items": { "type": "integer" } }
                        },
                        "required": ["id", "path", "title"]
                    }
                },
                {
                    "name": "delete_gallery",
                    "description": "Delete a gallery by ID.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "id": { "type": "integer" } },
                        "required": ["id"]
                    }
                }
            ]
        }),
    )
}
