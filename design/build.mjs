// Build / dev-server for the browser-live template preview.
//
//   node build.mjs            assemble ./preview/ (templates.json, fixtures.json,
//                             WASM, index.html, placeholder)
//   node build.mjs --serve    static dev server, templates regenerate live
//
// This tool lives in the design bundle (design/). Its build output lands in
// design/assets/preview/, which the *real* server serves at /assets/preview/* —
// it serves the whole assets/ folder under /assets/*. So the preview runs
// unchanged whether opened through the dev server below or straight off a
// running site at /assets/preview/index.html.
//
// Because the output is mounted under /assets/preview/ (not the web root), the
// page references its own assets RELATIVELY (./minijinja_js.js, ./templates.json,
// …) so it is base-path agnostic. The dev server mounts at the same /assets/
// preview prefix to match. Template resolution mirrors the Rust DesignStore
// (src/design.rs): an optional override (DESIGN_DIR / --design-dir) is preferred,
// falling back to this bundle's templates. `fetch` does not work over file://,
// so a static server is required either way.

import { createServer } from "node:http";
import { readdir, readFile, writeFile, mkdir, copyFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve, extname } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url)); // the design/ bundle root
const OUT_DIR = join(HERE, "assets", "preview"); // build destination (served at /assets/preview)
const RUNTIME_SRC = join(HERE, "node_modules", "minijinja-js", "dist", "web");
const RUNTIME_FILES = ["minijinja_js.js", "minijinja_js_bg.wasm"];
const COPY_FILES = ["index.html", "placeholder.svg"]; // source files copied verbatim
// Where the real server mounts the output (it serves assets/ under /assets/).
const MOUNT = "/assets/preview";

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

async function copyRuntime() {
  if (!existsSync(RUNTIME_SRC)) {
    throw new Error(
      `minijinja-js not found at ${RUNTIME_SRC}. Run \`npm install\` first.`,
    );
  }
  for (const file of RUNTIME_FILES) {
    await copyFile(join(RUNTIME_SRC, file), join(OUT_DIR, file));
  }
}

async function build() {
  const override = overrideDir();
  await mkdir(OUT_DIR, { recursive: true });
  const manifest = await buildManifest(override);
  await writeFile(join(OUT_DIR, "templates.json"), JSON.stringify(manifest, null, 2));
  await writeFile(join(OUT_DIR, "fixtures.json"), JSON.stringify(await loadFixtures(), null, 2));
  await copyRuntime();
  for (const file of COPY_FILES) await copyFile(join(HERE, file), join(OUT_DIR, file));
  console.log(
    `built ${OUT_DIR} — ${Object.keys(manifest).length} template(s)` +
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
  await build(); // populate design/preview/ (index.html, WASM, json, placeholder)
  const placeholder = join(OUT_DIR, "placeholder.svg");

  const server = createServer(async (req, res) => {
    try {
      const path = decodeURIComponent(new URL(req.url, "http://localhost").pathname);

      // Land on the same mount the real server uses, so the base path matches.
      if (path === "/" || path === "/index.html") {
        res.writeHead(302, { location: `${MOUNT}/index.html` });
        return res.end();
      }
      // Manifests regenerate per request so template/fixture edits show on reload.
      if (path === `${MOUNT}/templates.json`) {
        return send(res, 200, JSON.stringify(await buildManifest(override)), MIME[".json"]);
      }
      if (path === `${MOUNT}/fixtures.json`) {
        return send(res, 200, JSON.stringify(await loadFixtures()), MIME[".json"]);
      }
      // The preview's own assets (index.html, WASM, placeholder), as the real
      // server would serve them from design/preview/.
      if (path.startsWith(`${MOUNT}/`)) {
        return sendFile(res, join(OUT_DIR, path.slice(MOUNT.length + 1)));
      }
      // /assets/* → override → bundle (mirrors the real /assets route, which
      // maps to the bundle's assets/ folder). path.slice(1) keeps the "assets/"
      // prefix, e.g. "assets/css/style.css".
      if (path.startsWith("/assets/")) {
        const resolved = resolveAsset(path.slice(1), override);
        return resolved ? sendFile(res, resolved) : send(res, 404, "Not Found");
      }
      // /files/{hash}[/nahled] → bundled placeholder image.
      if (path.startsWith("/files/")) {
        return sendFile(res, placeholder);
      }
      send(res, 404, "Not Found");
    } catch (err) {
      send(res, 500, String(err && err.stack ? err.stack : err));
    }
  });

  server.listen(port, () => {
    console.log(`preview server on http://localhost:${port}/  ->  ${MOUNT}/index.html`);
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
