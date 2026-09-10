import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../..",
);
const result = spawnSync(
  process.execPath,
  [
    path.join(root, "web/node_modules/wasm-pack/run.js"),
    "build",
    path.join(root, "rust/crates/lobo_wasm"),
    "--target",
    "web",
    "--release",
    "--out-dir",
    path.join(root, "web/public/wasm"),
    "--out-name",
    "lobo_wasm",
  ],
  {
    cwd: root,
    stdio: "inherit",
    env: {
      ...process.env,
      CARGO_TARGET_DIR:
        process.env.CARGO_TARGET_DIR ?? path.join(root, "rust/target"),
    },
  },
);
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
