// Build / dev-server for the browser-live template preview.
//
//   node build.mjs            compile to design/preview/ (index.html,
//                             templates.json, fixtures.json) and copy the runtime
//                             libs into design/assets/{js,img}
//   node build.mjs --serve    static dev server, templates regenerate live
//
// This tool lives in the design bundle (design/). The compiler writes two kinds
// of output:
//   - the preview app (the page + its data) -> design/preview/
//   - the runtime libs it needs to run -> the served assets/ folders: the
//     minijinja-js runtime under assets/js, the placeholder image under
//     assets/img. The real /assets route serves those, so the page loads its
//     runtime from /assets/js/* just like any other static resource.
//
// The page therefore references its runtime with absolute /assets URLs and its
// own data relatively (./templates.json). Template resolution mirrors the Rust
// DesignStore (src/design.rs): an optional override (DESIGN_DIR / --design-dir)
// is preferred, falling back to this bundle's templates. `fetch` does not work
// over file://, so a static server is required either way.

import { createServer } from "node:http";
import { readdir, readFile, writeFile, mkdir, copyFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve, extname } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url)); // the design/ bundle root
// The compiled preview app (the page + its data) lands here.
const PREVIEW_DIR = join(HERE, "preview");
// Runtime libraries the page needs to run go into the served assets/ folders, so
// the real /assets route serves them (the minijinja-js runtime under assets/js,
// the placeholder image under assets/img).
const ASSETS_JS = join(HERE, "assets", "js");
const ASSETS_IMG = join(HERE, "assets", "img");
const RUNTIME_SRC = join(HERE, "node_modules", "minijinja-js", "dist", "web");
const RUNTIME_FILES = ["minijinja_js.js", "minijinja_js_bg.wasm"]; // -> assets/js

// --- argument / override resolution ----------------------------------------

function overrideDir() {
  const flag = process.argv.find((a) => a.startsWith("--design-dir="));
  const value = flag ? flag.slice("--design-dir=".length) : process.env.DESIGN_DIR;
  return value && value.length ? resolve(value) : null;
}

// Resolve a bundle-relative path (e.g. "css/style.css", "templates/base.html")
// against the override folder, falling back to this bundle. Returns an absolute
// path or null.
function resolveAsset(relPath, override) {
  if (override) {
    const candidate = join(override, relPath);
    if (existsSync(candidate)) return candidate;
  }
  const baked = join(HERE, relPath);
  return existsSync(baked) ? baked : null;
}

// --- template manifest ------------------------------------------------------

async function listTemplates(baseDir) {
  const root = join(baseDir, "templates");
  if (!existsSync(root)) return [];
  const out = [];
  async function walk(dir, prefix) {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const rel = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) await walk(join(dir, entry.name), rel);
      else if (entry.name.endsWith(".html")) out.push(rel);
    }
  }
  await walk(root, "");
  return out;
}

// Build { "<name>": "<raw source>" } preferring the override, falling back to
// this bundle — the union of template names across both layers.
async function buildManifest(override) {
  const names = new Set([
    ...(await listTemplates(HERE)),
    ...(override ? await listTemplates(override) : []),
  ]);
  const manifest = {};
  for (const name of [...names].sort()) {
    const path = resolveAsset(`templates/${name}`, override);
    if (path) manifest[name] = await readFile(path, "utf8");
  }
  return manifest;
}

async function loadFixtures() {
  const mod = await import(`./fixtures.mjs?t=${Date.now()}`);
  return mod.default;
}

// --- build mode -------------------------------------------------------------

// Copy the runtime libs into the served assets/ folders: minijinja-js js+wasm
// into assets/js, the placeholder image into assets/img.
async function copyRuntime() {
  if (!existsSync(RUNTIME_SRC)) {
    throw new Error(
      `minijinja-js not found at ${RUNTIME_SRC}. Run \`npm install\` first.`,
    );
  }
  await mkdir(ASSETS_JS, { recursive: true });
  await mkdir(ASSETS_IMG, { recursive: true });
  for (const file of RUNTIME_FILES) {
    await copyFile(join(RUNTIME_SRC, file), join(ASSETS_JS, file));
  }
  await copyFile(join(HERE, "placeholder.svg"), join(ASSETS_IMG, "placeholder.svg"));
}

async function build() {
  const override = overrideDir();
  await mkdir(PREVIEW_DIR, { recursive: true });
  const manifest = await buildManifest(override);
  await writeFile(join(PREVIEW_DIR, "templates.json"), JSON.stringify(manifest, null, 2));
  await writeFile(join(PREVIEW_DIR, "fixtures.json"), JSON.stringify(await loadFixtures(), null, 2));
  await copyFile(join(HERE, "index.html"), join(PREVIEW_DIR, "index.html"));
  await copyRuntime();
  console.log(
    `built ${PREVIEW_DIR} + runtime into assets/{js,img} — ` +
      `${Object.keys(manifest).length} template(s)` +
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
  await build(); // populate design/preview/ + runtime libs in assets/{js,img}
  const placeholder = join(ASSETS_IMG, "placeholder.svg");

  const server = createServer(async (req, res) => {
    try {
      const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);

      // Document root is the design bundle (/ => design/), like the live server.
      // The entry point is the compiled app at /preview/index.html.
      if (path === "/" || path === "/index.html") {
        res.writeHead(302, { location: "/preview/index.html" });
        return res.end();
      }
      // The app's data regenerates per request so template/fixture edits show on
      // reload (otherwise these would be served as the static files just built).
      if (path === "/preview/templates.json") {
        return send(res, 200, JSON.stringify(await buildManifest(override)), MIME[".json"]);
      }
      if (path === "/preview/fixtures.json") {
        return send(res, 200, JSON.stringify(await loadFixtures()), MIME[".json"]);
      }
      // /files/{hash}[/nahled] → bundled placeholder image.
      if (path.startsWith("/files/")) {
        return sendFile(res, placeholder);
      }
      // Everything else is served straight from the design bundle root
      // (override → bundle), exactly like a static server with docroot design/:
      // /preview/* (the app), /assets/* (runtime libs + css/js/img), …
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
