// Strips the auto-generated wasi fallback block from index.js after `napi build`.
//
// The @napi-rs/cli node loader template unconditionally falls back to
// require('@alidade/query-parser-wasm32-wasi') when the native binding fails to
// load. Turbopack's file tracer follows that require into a virtual
// [turbopack-wasm] module and crashes Next.js 16.3 builds on Vercel
// ("NftJsonAsset: cannot handle filepath '[turbopack-wasm]/node/loadWasm.ts'").
// Node consumers always have a real native binding (it's in optionalDependencies),
// and browsers use browser.js, so the fallback is dead code in practice.
// CI's wasi test loads the binding via NAPI_RS_NATIVE_LIBRARY_PATH instead.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const indexPath = fileURLToPath(new URL("../index.js", import.meta.url));
const source = readFileSync(indexPath, "utf8");

const startMarker = "if (!nativeBinding || process.env.NAPI_RS_FORCE_WASI) {";
const endMarker = "if (!nativeBinding) {";

const start = source.indexOf(startMarker);
if (start === -1) {
  console.log("strip-wasi-fallback: no wasi fallback block found, nothing to do");
  process.exit(0);
}
const end = source.indexOf(endMarker, start);
if (end === -1) {
  throw new Error("strip-wasi-fallback: found start marker but no end marker — template changed, update this script");
}
const removed = source.slice(start, end);
if (!removed.includes("wasm32-wasi")) {
  throw new Error("strip-wasi-fallback: block between markers does not look like the wasi fallback — template changed, update this script");
}

writeFileSync(indexPath, source.slice(0, start) + source.slice(end));
console.log(`strip-wasi-fallback: removed ${removed.length} chars from index.js`);
