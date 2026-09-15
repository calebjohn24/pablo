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
  assert.match(html, /A 15× smaller harness for your agents/);
  assert.doesNotMatch(html, /measured footprint/i);
  assert.match(html, /The agent runtime your application can own/);
  assert.match(html, /<title>Pablo — A 15× smaller harness for your agents<\/title>/);
  assert.match(html, /property="og:title" content="Pablo — A 15× smaller harness for your agents"/);
  assert.match(html, /name="twitter:title" content="Pablo — A 15× smaller harness for your agents"/);
  assert.doesNotMatch(html, /Pablo — Rust agent runtime/);
  assert.match(html, /non-coding knowledge work/i);
  assert.match(html, /One small Rust binary\. The agent stack is built in\./);
  assert.match(html, /OPEN PROTOCOLS \/ INCLUDED/);
  assert.match(html, /ACP/);
  assert.match(html, /MCP/);
  assert.match(html, /A2A/);
  assert.match(html, /Agent Skills/);
  assert.doesNotMatch(html, /Runtime overhead, measured/);
  assert.match(html, /RUNTIME RESOURCE USE/);
  assert.doesNotMatch(html, /benchmark-figure-prominent/);
  assert.match(html, /ACP application host/);
  assert.match(html, /Install and integrate/);
  assert.match(html, /Try Pablo today/);
  assert.doesNotMatch(html, /Prerelease status/);
  assert.doesNotMatch(html, /Ready to add agent execution/i);
  assert.doesNotMatch(html, /exploratory single-seed pilot/i);
  assert.doesNotMatch(html, /codex-preview|Starter Project|Your site is taking shape/);
});

test("getting started leads application hosts from installation to embedding", () => {
  const html = fs.readFileSync(path.join(dist, "getting-started.html"), "utf8");
  const source = fs.readFileSync(path.join(guideRoot, "getting-started.md"), "utf8");
  assert.match(html, /Choose an integration boundary/);
  assert.match(html, /Install Pablo/);
  assert.match(html, /runpablo\.pages\.dev\/install\.sh/);
  assert.match(html, /pablo acp --stdio/);
  assert.match(html, /PABLO_BINARY/);
  assert.match(html, /pablo-core/);
  assert.match(html, /Define production authority/);
  assert.match(source, /curl -fsSL https:\/\/runpablo\.pages\.dev\/install\.sh \| sh/);
  assert(html.indexOf("Install Pablo") < html.indexOf("Verify the installation offline"));
});

test("subagent guide documents bounded local and remote delegation", () => {
  const html = fs.readFileSync(path.join(dist, "subagents.html"), "utf8");
  const source = fs.readFileSync(path.join(guideRoot, "subagents.md"), "utf8");
  assert.match(html, /Enable local subagents/);
  assert.match(html, /Validated handoffs/);
  assert.match(source, /options\.children/);
  assert.match(source, /spawn_remote/);
  assert.match(source, /two active children/i);
  assert.match(source, /depth one/i);
  assert.match(source, /A2A client/);
});

test("resource benchmark publishes measured values and its graphic", () => {
  const html = fs.readFileSync(path.join(dist, "benchmarks.html"), "utf8");
  const desktopGraphic = fs.readFileSync(
    path.join(guideRoot, "assets", "resource-usage.svg"),
    "utf8",
  );
  const mobileGraphic = fs.readFileSync(
    path.join(guideRoot, "assets", "resource-usage-mobile.svg"),
    "utf8",
  );
  assert.match(html, /Resource benchmark/);
  assert.match(html, /19\.11 MiB/);
  assert.match(html, /286\.09 MiB/);
  assert.match(html, /CPU utilization p50/);
  assert.match(html, /one seed and one attempt/i);
  assert.doesNotMatch(desktopGraphic, /exploratory single-seed pilot/i);
  assert.doesNotMatch(mobileGraphic, /exploratory single-seed pilot/i);
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
  assert(fs.existsSync(path.join(dist, "social/benchmark-15x.png")));
  assert(fs.existsSync(path.join(dist, "social/benchmark-15x-footprint.png")));
  assert(fs.existsSync(path.join(dist, "social/logos/pablo.svg")));
  assert(fs.existsSync(path.join(dist, "social/logos/claude.svg")));
  assert(fs.existsSync(path.join(dist, "social/logos/codex.svg")));
  const og = fs.readFileSync(path.join(dist, "og.png"));
  assert.equal(og.readUInt32BE(16), 1200, "OG image width must be 1200 pixels");
  assert.equal(og.readUInt32BE(20), 630, "OG image height must be 630 pixels");
  const social = fs.readFileSync(path.join(dist, "social/benchmark-15x-footprint.png"));
  assert.equal(social.readUInt32BE(16), 1200, "social benchmark width must be 1200 pixels");
  assert.equal(social.readUInt32BE(20), 630, "social benchmark height must be 630 pixels");
  assert.deepEqual(
    fs.readFileSync(path.join(dist, "social/benchmark-15x.png")),
    social,
    "original social URL must serve the corrected footprint card",
  );
  const socialSource = fs.readFileSync(path.resolve("public/social/benchmark-15x-footprint.svg"), "utf8");
  assert.match(socialSource, /CLAUDE CODE \+ CODEX/);
  assert.match(socialSource, /15×.*SMALLER.*FOOTPRINT/s);
  assert.doesNotMatch(socialSource, /MORE CPU/);
  assert.doesNotMatch(socialSource, /six-task workload/i);
  assert.doesNotMatch(socialSource, /6 SYNTHETIC TASKS/);
  assert.match(socialSource, /15\.1× CPU/);
  assert.match(socialSource, /16\.3× CPU/);
  assert.match(socialSource, /logos\/pablo\.svg/);
  assert.match(socialSource, /logos\/claude\.svg/);
  assert.match(socialSource, /logos\/codex\.svg/);
  assert.match(fs.readFileSync(path.resolve("public/social/logos/pablo.svg"), "utf8"), /#ff6b3a/);
  assert.match(fs.readFileSync(path.resolve("public/social/logos/claude.svg"), "utf8"), /#d97757/);
  assert.match(fs.readFileSync(path.resolve("public/social/logos/codex.svg"), "utf8"), /#10a37f/);
  const ogSource = fs.readFileSync(path.resolve("public/og.svg"), "utf8");
  assert.match(ogSource, /Pablo — A 15× smaller harness for your agents/);
  assert.match(ogSource, /15×/);
  assert.match(ogSource, /CLAUDE CODE/);
  assert.doesNotMatch(ogSource, /agent runtime your application can own/i);
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
  const filename = "pablo-v0.0.1-aarch64-apple-darwin.tar.gz";
  const response = await request(`v0.0.1/${filename}`);
  assert.equal(await response.text(), "archive bytes");
  assert.deepEqual(calls, [`v0.0.1/${filename}`]);
  assert.equal(response.headers.get("cache-control"), "public, max-age=31536000, immutable");
  assert.equal(response.headers.get("content-type"), "application/gzip");
  assert.equal((await request(`v0.0.2/${filename}`)).status, 404);
  assert.equal((await request("../secret")).status, 404);
  assert.equal((await request(`v0.0.1/${filename}`, "POST")).status, 405);
  const head = await request(`v0.0.1/${filename}`, "HEAD");
  assert.equal(await head.text(), "");
});
