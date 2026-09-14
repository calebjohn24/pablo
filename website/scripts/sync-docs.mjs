import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const siteRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const source = path.resolve(siteRoot, "../docs/guide");
const target = path.resolve(siteRoot, ".content");
const publicSource = path.resolve(siteRoot, "public");
const publicTarget = path.resolve(siteRoot, ".public");

if (!target.startsWith(`${siteRoot}${path.sep}`)) {
  throw new Error("Refusing to write generated content outside the website");
}
if (!publicTarget.startsWith(`${siteRoot}${path.sep}`)) {
  throw new Error("Refusing to write generated assets outside the website");
}
if (!fs.statSync(source).isDirectory()) {
  throw new Error(`Missing documentation source: ${source}`);
}

fs.rmSync(target, { recursive: true, force: true });
fs.cpSync(source, target, { recursive: true });
fs.writeFileSync(
  path.join(target, "tsconfig.json"),
  `${JSON.stringify(
    {
      compilerOptions: {
        target: "ES2022",
        module: "ESNext",
        moduleResolution: "Bundler",
        strict: true,
        skipLibCheck: true,
      },
    },
    null,
    2,
  )}\n`,
);
fs.rmSync(publicTarget, { recursive: true, force: true });
fs.cpSync(publicSource, publicTarget, { recursive: true });
fs.cpSync(path.join(source, "examples"), path.join(publicTarget, "examples"), {
  recursive: true,
});
