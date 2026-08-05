// Wraps `napi build --platform`, forwarding CLI args (e.g. --target, --release,
// --use-napi-cross from CI), then strips the wasi fallback from the regenerated
// index.js. Needed because a plain `napi build && node strip` script would make
// pnpm append extra args to the strip command instead of napi build.
import { spawnSync } from "node:child_process";

const result = spawnSync("napi", ["build", "--platform", ...process.argv.slice(2)], {
  stdio: "inherit",
  shell: process.platform === "win32",
});
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}
await import("./strip-wasi-fallback.mjs");
