import test from "ava";
import { parse, validate, isValid, format } from "../index.js";

test("parse valid query", (t) => {
  const result = parse("apple OR orange");
  t.true(result.ok);
  t.is(result.diagnostics.items.length, 0);
  t.is(result.stats?.termCount, 2);
});

test("parse invalid query", (t) => {
  const result = parse("title:");
  t.false(result.ok);
  t.true(result.diagnostics.items.length > 0);
  t.is(result.diagnostics.items[0]?.message, "expected word");
});

test("isValid", (t) => {
  t.true(isValid("foo AND bar"));
  t.false(isValid("field:"));
});

test("format normalizes query", (t) => {
  t.is(format("foo"), "foo");
});

test("validate returns diagnostics", (t) => {
  t.is(validate("foo AND bar").items.length, 0);
  t.true(validate("field:").items.length > 0);
});
