// Build / dev-server for the static template preview.
//
//   node build.mjs            render every target template to its own HTML file
//                             in design/preview/ (+ a static index linking them)
//   node build.mjs --serve    dev server that re-renders on each request
//
// This tool lives in the design bundle (design/). Rendering happens here, in
// Node, with minijinja-js — there is no router and no in-browser WASM. Each
// render target becomes a standalone, ready-to-open page in design/preview/:
//   page.html  menu.html  search.html  404.html  (+ index.html, a link list)
//
// The pages are mount-agnostic: the absolute /assets and /files URLs the
// production templates emit are rewritten to paths relative to design/preview/
// (../assets/…), so a page resolves its assets whether the bundle is served at
// the web root or under a prefix (e.g. design/ => /raw/). Template resolution
// mirrors the Rust DesignStore (src/design.rs): an optional override (DESIGN_DIR
// / --design-dir) is preferred, falling back to this bundle's templates.

import { createServer } from "node:http";
import { readFile, writeFile, mkdir, copyFile } from "node:fs/promises";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve, extname } from "node:path";
import { Environment } from "minijinja-js/dist/node/minijinja_js.js";

const HERE = dirname(fileURLToPath(import.meta.url)); // the design/ bundle root
const PREVIEW_DIR = join(HERE, "preview"); // rendered pages land here
const ASSETS_IMG = join(HERE, "assets", "img"); // placeholder image lives here

// --- override resolution ----------------------------------------------------

function overrideDir() {
  const flag = process.argv.find((a) => a.startsWith("--design-dir="));
  const value = flag ? flag.slice("--design-dir=".length) : process.env.DESIGN_DIR;
  return value && value.length ? resolve(value) : null;
}

// Resolve a bundle-relative path (e.g. "templates/base.html") against the
// override folder, falling back to this bundle. Returns an absolute path or null.
function resolveAsset(relPath, override) {
  if (override) {
    const candidate = join(override, relPath);
    if (existsSync(candidate)) return candidate;
  }
  const baked = join(HERE, relPath);
  return existsSync(baked) ? baked : null;
}

async function loadFixtures() {
  const mod = await import(`./fixtures.mjs?t=${Date.now()}`);
  return mod.default;
}

// --- rendering --------------------------------------------------------------

// minijinja-js cannot register custom JS filters. The only custom filter the
// templates use is `timeformat` (src/state.rs); strip it out as templates load
// and let the fixtures carry pre-formatted date strings.
const stripTimeformat = (src) => src.replace(/\|\s*timeformat\s*(\([^)]*\))?/g, "");

// Rewrite the absolute asset URLs the production templates emit so each rendered
// page (in design/preview/) resolves them relative to itself: /assets/* becomes
// ../assets/* (mount-agnostic); /files/* (real uploads we don't have) collapses
// to the bundled placeholder image.
const rewriteAssets = (html) =>
  html
    .replace(/(["'(])\/assets\//g, "$1../assets/")
    .replace(/\/files\/[^"'\s)>]+/g, "../assets/img/placeholder.svg");

// A fresh environment whose loader reads templates from disk (override → bundle)
// on each render, so edits show up without restarting.
function makeEnv(override) {
  const env = new Environment();
  // Mirror the Rust server, which uses MiniJinja's default (lenient) undefined
  // handling — e.g. the menu-page fixture omits `page`.
  env.undefinedBehavior = "lenient";
  env.setLoader((name) => {
    const path = resolveAsset(`templates/${name}`, override);
    return path ? stripTimeformat(readFileSync(path, "utf8")) : null;
  });
  return env;
}

// Recursively compose a body block tree into HTML by rendering the
// markdown/*.html directive templates — the directive loopback as data.
function renderBlocks(env, blocks) {
  return (blocks || [])
    .map((b) => {
      if (b.type === "prose") return b.html;
      if (b.type === "page") {
        const inner_html = renderBlocks(env, b.body);
        return env.renderTemplate("markdown/page.html", { path: b.path, inner_html });
      }
      return env.renderTemplate(`markdown/${b.name}.html`, b.ctx);
    })
    .join("\n");
}

function renderTarget(env, target) {
  const ctx = { ...target.context };
  if (target.body) ctx.body_html = renderBlocks(env, target.body);
  return rewriteAssets(env.renderTemplate(target.template, ctx));
}

// A plain static index linking the rendered pages (no JS, no router).
function landingHtml(fixtures) {
  const rows = Object.values(fixtures.targets)
    .map(
      (t) =>
        `      <li><a href="./${t.file}">${t.label}</a> ` +
        `<code>${t.template}</code></li>`,
    )
    .join("\n");
  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Template preview</title>
  <style>
    body { font: 15px/1.6 system-ui, sans-serif; max-width: 40rem; margin: 3rem auto; padding: 0 1rem; }
    h1 { font-size: 1.4rem; } li { margin: .35rem 0; } code { color: #6b7178; }
  </style>
</head>
<body>
  <h1>Template preview</h1>
  <p>Production MiniJinja templates rendered with dummy data. Each is a standalone page:</p>
  <ul>
${rows}
  </ul>
</body>
</html>
`;
}

// --- build mode -------------------------------------------------------------

async function build() {
  const override = overrideDir();
  await mkdir(PREVIEW_DIR, { recursive: true });
  const fixtures = await loadFixtures();
  const env = makeEnv(override);
  let count = 0;
  for (const target of Object.values(fixtures.targets)) {
    await writeFile(join(PREVIEW_DIR, target.file), renderTarget(env, target));
    count += 1;
  }
  await writeFile(join(PREVIEW_DIR, "index.html"), landingHtml(fixtures));
  await mkdir(ASSETS_IMG, { recursive: true });
  await copyFile(join(HERE, "placeholder.svg"), join(ASSETS_IMG, "placeholder.svg"));
  console.log(
    `built ${PREVIEW_DIR} — ${count} page(s)` +
      (override ? `, override: ${override}` : ", bundle only"),
  );
}

// --- serve mode -------------------------------------------------------------

const MIME = {
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".css": "text/css",
  ".html": "text/html; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".ico": "image/x-icon",
};

const mimeFor = (path) => MIME[extname(path).toLowerCase()] || "application/octet-stream";

function send(res, status, body, type) {
  res.writeHead(status, {
    "content-type": type || "text/plain; charset=utf-8",
    "cache-control": "no-store",
  });
  res.end(body);
}

async function sendFile(res, path) {
  try {
    send(res, 200, await readFile(path), mimeFor(path));
  } catch {
    send(res, 404, "Not Found");
  }
}

async function serve() {
  const override = overrideDir();
  const port = Number(process.env.PORT) || 4321;
  await build(); // write design/preview/ once + copy the placeholder

  const server = createServer(async (req, res) => {
    try {
      const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);

      // Document root is the design bundle (/ => design/), like the live server.
      if (path === "/" || path === "/index.html") {
        res.writeHead(302, { location: "/preview/index.html" });
        return res.end();
      }
      // The preview pages re-render per request so template/fixture edits show on
      // reload (instead of serving the static files written at startup).
      if (path.startsWith("/preview/")) {
        const name = path.slice("/preview/".length);
        const fixtures = await loadFixtures();
        if (name === "" || name === "index.html") {
          return send(res, 200, landingHtml(fixtures), MIME[".html"]);
        }
        const target = Object.values(fixtures.targets).find((t) => t.file === name);
        if (target) {
          return send(res, 200, renderTarget(makeEnv(override), target), MIME[".html"]);
        }
        // fall through to static for anything else under /preview/
      }
      // Everything else is served straight from the design bundle root
      // (override → bundle): /assets/* (css/js/img), etc.
      const rel = path.slice(1);
      if (rel.split("/").includes("..")) return send(res, 403, "Forbidden");
      const resolved = resolveAsset(rel, override);
      return resolved ? sendFile(res, resolved) : send(res, 404, "Not Found");
    } catch (err) {
      send(res, 500, String(err && err.stack ? err.stack : err));
    }
  });

  server.listen(port, () => {
    console.log(`preview server on http://localhost:${port}/  ->  /preview/index.html`);
    console.log(override ? `override: ${override}` : "bundle only");
  });
}

// --- entry ------------------------------------------------------------------

try {
  await (process.argv.includes("--serve") ? serve() : build());
} catch (err) {
  console.error(err.message || err);
  process.exit(1);
}
