// TEST_WASI=1 tests the wasm binding directly (index.js no longer falls back
// to wasi — see scripts/strip-wasi-fallback.mjs). A loader-level override via
// NAPI_RS_NATIVE_LIBRARY_PATH doesn't work here: it's global to every napi-rs
// package in the process, including ava's own TS loader.
module.exports = process.env.TEST_WASI
  ? require("../query-parser.wasi.cjs")
  : require("../index.js");
