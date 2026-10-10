# Design bundle

The baked default design of the public site, embedded into `site_server` via
`rust-embed` (`src/design/`). A deployment overrides it per file from storage;
this folder is the fallback and the starting point.

## Layout

Only these three roots are deployable — anything else in this folder is never
served, published or accepted by `site_cli design push`:

| Root | Role |
|---|---|
| `templates/` | MiniJinja templates of the public pages (`base.html`, `path_page.html`, `page_search.html`, `404.html`) and of the markdown directives (`markdown/*.html`) |
| `assets/` | Static files served under `/assets/*` (`css/`, `js/`, `img/`) |
| `mdcast/` | Export brand config (`brand.toml`) and typst / reveal.js overrides for PDF and slides exports |

## Resolution order

Every path (`templates/X`, `assets/X`, `mdcast/X`) resolves to the first of:

1. `DESIGN_DIR/<path>` — dev-only local folder (debug builds read it live,
   release builds freeze it at startup).
2. The **published** design in storage (`design/<path>` keys).
3. This baked bundle.

See [Design overrides](../docs/architecture.md#design-overrides).

## Editing workflow

Deployments keep their design in storage, edited through one shared **draft**
in the admin Design page:

- **Draft** — any admin edits it (`/api/design/draft/*`); it is invisible to
  visitors. A path the draft does not hold shows its baked default, so removing
  a file from the draft reverts it.
- **Preview** — turn on draft preview to browse the real site rendered with the
  draft (session-only, with a banner and a template error page). See
  [Draft preview](../docs/architecture.md#draft-preview).
- **Publish** — validates the draft (template compile + a strict smoke render
  against the site's real data), snapshots it to the history and makes it live
  atomically.
- **History** — every published version is kept; restoring one copies it into
  the draft, to be published again.

Other ways in:

- `site_cli design push <dir>` uploads a folder in this layout **into the
  draft** (paths outside the three roots are skipped); publish it from the
  admin.
- AI designer: the in-house assistant and external agents over MCP get
  `design_*` tools that read and write the draft only (#118) — publishing stays
  a human action. See the MCP tool list in
  [`docs/architecture.md`](../docs/architecture.md).
- Bucket edits of `design/` objects + **Reload** in the admin (bypasses the
  draft; the next publish reports the conflict).

## Template contract

What each template receives is generated from the typed context structs:
[`docs/design-contract.md`](../docs/design-contract.md) (and its JSON Schema
`docs/design-contract.schema.json`). Regenerate both with `make contract`
after changing a context; a unit test fails when they drift.
