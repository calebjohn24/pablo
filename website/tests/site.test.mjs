import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

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
  assert.doesNotThrow(() =>
    JSON.parse(fs.readFileSync(path.join(guideRoot, "examples/review.schema.json"), "utf8")),
  );
});
