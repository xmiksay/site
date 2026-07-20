---
name: site-mcp
description: Test and exercise this site's MCP endpoint (the `POST /mcp` JSON-RPC server). Use when verifying MCP tools work after a change, calling page_read/page_search/page_edit/file/gallery tools over MCP, or deciding between the local test target and the remote production target. Enforces local-for-testing / remote-is-the-real-server safety. (Named site-mcp to avoid the built-in /mcp command.)
---

# /site-mcp — exercise the site MCP endpoint

The site **is** an MCP server (`POST /mcp`, JSON-RPC 2.0, Bearer auth — service token or OAuth access token). Two targets — pick deliberately, default to **local**.

## Two targets

- **Local — for testing.** `http://localhost:3000/mcp` — a `make run` instance (binds `PORT=3000`, needs `DATABASE_URL`). This is where you confirm a change works: exercise tools freely, including writes, against throwaway paths (e.g. `scratch/mcp-test`).
- **Remote — the real server (production).** `https://miksanik.net/mcp` — the **live** public site. ⚠️ The committed-but-gitignored `.mcp.json` in this repo points **here**, so the in-session `site` MCP tools hit production. Treat as **read-only by default**: any write (`page_edit`, `page_delete`, `create/update/delete` for tags/files/galleries) mutates the live site — **confirm explicitly before each call**, state the exact path/scope first, and never run test/scratch operations here.

Identify the target before any call; when ambiguous, ask. Never point a verification run at production.

## Verify the MCP surface (local)

After changing an MCP tool:

1. **Bring it up.** `make run` (server on :3000), or confirm it's already serving.
2. **`tools/list`.** Call it over `/mcp` and confirm the set matches the tools defined in `src/routes/mcp/{pages,tags,files,galleries}.rs` — Pages (`page_read`/`page_edit`/`page_search`/`page_delete`), Tags, Files, Galleries (list/read/create/update/delete each). Tool/param descriptions come from `handle_tools_list()` (`src/routes/mcp/instructions.rs`).
3. **Round-trip the changed tool** against a throwaway path — e.g. `page_search` → `page_read`, or `page_edit` on `scratch/mcp-test` then read it back.
4. **Auth & audit.** `Authorization: Bearer <token>` accepts a service token or an OAuth2 access token; the handler resolves it to a `user_id` for audit fields. Verify an unauthenticated call is rejected.
5. **Report** the `tools/list` result, the round-trip outcome, and the auth check. Never claim green on red.

## Notes

- Server instructions load from a private `CLAUDE` page if present (editable in the admin UI), else `server_instructions()` in `src/routes/mcp/instructions.rs`.
- `.mcp.json` is gitignored — it's your local connection (URL + token). Don't commit it or paste the token. To test locally, point it at `http://localhost:3000/mcp`; to operate on the real site, leave it on `https://miksanik.net/mcp` and follow the read-only-by-default rule above.
- Production writes are real and public — confirm scope first, like the `/git` outward-facing rule (page edits keep revisions; deletes do not).
