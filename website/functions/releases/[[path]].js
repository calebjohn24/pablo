const VERSION = /^v[0-9A-Za-z][0-9A-Za-z._-]*$/;
const FILE = /^(?:pablo-v[0-9A-Za-z._-]+-(?:aarch64-apple-darwin|x86_64-apple-darwin|aarch64-unknown-linux-gnu|x86_64-unknown-linux-gnu)\.tar\.gz(?:\.sha256)?|SHA256SUMS|release-manifest\.json)$/;

function error(status, message) {
  return new Response(`${message}\n`, {
    status,
    headers: {
      "Cache-Control": "no-store",
      "Content-Type": "text/plain; charset=utf-8",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

function pathSegments(value) {
  if (Array.isArray(value)) return value;
  return typeof value === "string" ? value.split("/").filter(Boolean) : [];
}

export async function onRequest(context) {
  if (context.request.method !== "GET" && context.request.method !== "HEAD") {
    return error(405, "Method not allowed");
  }
  const segments = pathSegments(context.params.path);
  if (segments.length !== 2 || !VERSION.test(segments[0]) || !FILE.test(segments[1])) {
    return error(404, "Release file not found");
  }
  const [version, filename] = segments;
  if (filename.startsWith("pablo-") && !filename.startsWith(`pablo-${version}-`)) {
    return error(404, "Release file not found");
  }
  const object = await context.env.PABLO_RELEASES.get(`${version}/${filename}`);
  if (object === null) return error(404, "Release file not found");

  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("Cache-Control", "public, max-age=31536000, immutable");
  headers.set("Content-Disposition", `attachment; filename="${filename}"`);
  headers.set("ETag", object.httpEtag);
  headers.set("X-Content-Type-Options", "nosniff");
  if (!headers.has("Content-Type")) {
    headers.set("Content-Type", filename.endsWith(".json")
      ? "application/json; charset=utf-8"
      : filename.endsWith(".tar.gz")
        ? "application/gzip"
        : "text/plain; charset=utf-8");
  }
  return new Response(context.request.method === "HEAD" ? null : object.body, { headers });
}
