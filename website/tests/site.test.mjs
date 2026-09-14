import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { onRequest as releaseDownload } from "../functions/releases/[[path]].js";

const dist = path.resolve("dist");
const guideRoot = path.resolve("../docs/guide");

test("build contains landing page and every public guide", () => {
  const guides = fs
    .readdirSync(guideRoot)
    .filter((file) => file.endsWith(".md"));
  assert(guides.length >= 12, "expected the complete guide set");
  for (const guide of guides) {
    const route = guide === "index.md" ? "index.html" : `${guide.slice(0, -3)}.html`;
    assert(fs.existsSync(path.join(dist, route)), `missing ${route}`);
  }
});

test("generated docs exactly match GitHub source and local links resolve", () => {
  const guides = fs.readdirSync(guideRoot).filter((file) => file.endsWith(".md"));
  for (const guide of guides) {
    const source = fs.readFileSync(path.join(guideRoot, guide), "utf8");
    const generated = fs.readFileSync(path.resolve(".content", guide), "utf8");
    assert.equal(generated, source, `${guide} differs from its generated copy`);
    assert.doesNotMatch(source, /arbitrary work/i);
    for (const match of source.matchAll(/\]\(([^)]+)\)/g)) {
      const href = match[1];
      if (!href.startsWith(".")) continue;
      const target = href.split("#")[0];
      assert(
        fs.existsSync(path.resolve(guideRoot, path.dirname(guide), target)),
        `${guide} has a broken link to ${href}`,
      );
    }
  }
});

test("landing page carries product copy and no starter metadata", () => {
  const html = fs.readFileSync(path.join(dist, "index.html"), "utf8");
  assert.match(html, /The agent runtime your application can own/);
  assert.match(html, /Pablo — Rust agent runtime/);
  assert.match(html, /non-coding knowledge work/i);
  assert.match(html, /Local runtime overhead, measured/);
  assert.match(html, /resource-usage/);
  assert.doesNotMatch(html, /codex-preview|Starter Project|Your site is taking shape/);
});

test("resource benchmark publishes measured values and its graphic", () => {
  const html = fs.readFileSync(path.join(dist, "benchmarks.html"), "utf8");
  assert.match(html, /Resource benchmark/);
  assert.match(html, /19\.11 MiB/);
  assert.match(html, /286\.09 MiB/);
  assert.match(html, /CPU utilization p50/);
  assert.match(html, /one seed and one attempt/i);
  assert(
    fs.readdirSync(path.join(dist, "assets")).some((file) =>
      /^resource-usage\..+\.svg$/.test(file),
    ),
    "missing resource benchmark SVG",
  );
  assert(
    fs.readdirSync(path.join(dist, "assets")).some((file) =>
      /^resource-usage-mobile\..+\.svg$/.test(file),
    ),
    "missing mobile resource benchmark SVG",
  );
});

test("Cloudflare Pages assets and headers are emitted", () => {
  assert(fs.existsSync(path.join(dist, "_headers")));
  assert(fs.existsSync(path.join(dist, "favicon.svg")));
  assert(fs.existsSync(path.join(dist, "apple-touch-icon.png")));
  assert(fs.existsSync(path.join(dist, "og.png")));
  const og = fs.readFileSync(path.join(dist, "og.png"));
  assert.equal(og.readUInt32BE(16), 1200, "OG image width must be 1200 pixels");
  assert.equal(og.readUInt32BE(20), 630, "OG image height must be 630 pixels");
  const home = fs.readFileSync(path.join(dist, "index.html"), "utf8");
  assert.match(home, /href="\/favicon\.svg"/);
  assert.match(home, /https:\/\/runpablo\.pages\.dev\/og\.png/);
  assert(fs.existsSync(path.join(dist, "sitemap.xml")));
  assert(fs.existsSync(path.join(dist, "examples/vercel.toml")));
  assert(fs.existsSync(path.join(dist, "examples/open-responses.toml")));
  assert(fs.existsSync(path.join(dist, "examples/review.schema.json")));
  assert(fs.existsSync(path.join(dist, "install.sh")));
  assert.equal(
    fs.readFileSync(path.join(dist, "install.sh"), "utf8"),
    fs.readFileSync(path.resolve("../install.sh"), "utf8"),
    "published installer differs from the repository source",
  );
  assert.doesNotThrow(() =>
    JSON.parse(fs.readFileSync(path.join(guideRoot, "examples/review.schema.json"), "utf8")),
  );
});

test("release downloads are restricted to immutable versioned R2 objects", async () => {
  const calls = [];
  const object = {
    body: "archive bytes",
    httpEtag: '"fixture-etag"',
    writeHttpMetadata(headers) {
      headers.set("Content-Type", "application/gzip");
    },
  };
  const request = (path, method = "GET") => releaseDownload({
    request: new Request(`https://runpablo.pages.dev/releases/${path}`, { method }),
    params: { path: path.split("/") },
    env: {
      PABLO_RELEASES: {
        async get(key) {
          calls.push(key);
          return object;
        },
      },
    },
  });
  const filename = "pablo-v0.1.0-dev.1-aarch64-apple-darwin.tar.gz";
  const response = await request(`v0.1.0-dev.1/${filename}`);
  assert.equal(await response.text(), "archive bytes");
  assert.deepEqual(calls, [`v0.1.0-dev.1/${filename}`]);
  assert.equal(response.headers.get("cache-control"), "public, max-age=31536000, immutable");
  assert.equal(response.headers.get("content-type"), "application/gzip");
  assert.equal((await request(`v0.1.0-dev.2/${filename}`)).status, 404);
  assert.equal((await request("../secret")).status, 404);
  assert.equal((await request(`v0.1.0-dev.1/${filename}`, "POST")).status, 405);
  const head = await request(`v0.1.0-dev.1/${filename}`, "HEAD");
  assert.equal(await head.text(), "");
});
