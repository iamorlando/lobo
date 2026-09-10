// Package the same App Router UI as static files, served directly by Axum.
// API routes belong to the Rust host in this deployment; no Node server is needed.
import { cp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const web = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const stage = path.join(web, ".server-build");
const destination = path.resolve(web, "../python/lobo/_web");
await rm(stage, { recursive: true, force: true });
await mkdir(path.join(stage, "app"), { recursive: true });
for (const directory of ["components", "lib", "public"]) {
  await cp(path.join(web, directory), path.join(stage, directory), {
    recursive: true,
  });
}
for (const file of [
  "page.tsx",
  "layout.tsx",
  "globals.css",
  "design-tokens.css",
]) {
  await cp(path.join(web, "app", file), path.join(stage, "app", file));
}
await cp(path.join(web, "package.json"), path.join(stage, "package.json"));
const tsconfig = JSON.parse(
  await readFile(path.join(web, "tsconfig.json"), "utf8"),
);
await writeFile(path.join(stage, "tsconfig.json"), JSON.stringify(tsconfig));
await symlink(
  path.join(web, "node_modules"),
  path.join(stage, "node_modules"),
  process.platform === "win32" ? "junction" : "dir",
);
await writeFile(
  path.join(stage, "next.config.mjs"),
  `export default ${JSON.stringify({
    output: "export",
    devIndicators: false,
    agentRules: false,
    turbopack: { root: web },
    outputFileTracingRoot: web,
  })};\n`,
);
execFileSync(
  process.execPath,
  [path.join(web, "node_modules/next/dist/bin/next"), "build", stage],
  {
    cwd: web,
    stdio: "inherit",
  },
);
await rm(destination, { recursive: true, force: true });
await cp(path.join(stage, "out"), destination, { recursive: true });
console.log(`Packaged terminal assets: ${destination}`);
