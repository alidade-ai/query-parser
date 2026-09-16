import test from "ava";
import binding from "./binding.cjs";

const { parse, validate, isValid, format, toTsquery } = binding;

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

test("toTsquery emits a single-field match", (t) => {
  const result = toTsquery('"health care" AND policy');
  t.true(result.ok);
  t.deepEqual(JSON.parse(result.expression ?? ""), {
    type: "match",
    field: "content_exact",
    tsquery: "'health care' & 'policy'",
  });
});

test("toTsquery splits fields into an expression tree", (t) => {
  const result = toTsquery("content:apple AND banana", {
    allowedFields: ["content", "content_exact"],
  });
  t.true(result.ok);
  const expr = JSON.parse(result.expression ?? "");
  t.is(expr.type, "and");
  t.deepEqual(expr.children[0], {
    type: "match",
    field: "content",
    tsquery: "'apple'",
  });
});

test("toTsquery rejects wildcard queries", (t) => {
  const result = toTsquery("appl*");
  t.false(result.ok);
  t.is(result.expression, undefined);
});

test("toTsquery treats AND/OR followed by a newline, CRLF, or tab as operators", (t) => {
  for (const sep of ["\n", "\r\n", "\t"]) {
    const and = toTsquery(`(test OR test)${sep}AND${sep}(testing OR testing)`);
    t.true(and.ok, JSON.stringify(sep));
    t.is(and.diagnostics.items.length, 0, JSON.stringify(sep));
    t.is(
      JSON.parse(and.expression ?? "").tsquery,
      "('test' | 'test') & ('testing' | 'testing')",
      JSON.stringify(sep),
    );

    const or = toTsquery(`apple OR${sep}banana`);
    t.true(or.ok, JSON.stringify(sep));
    t.is(
      JSON.parse(or.expression ?? "").tsquery,
      "'apple' | 'banana'",
      JSON.stringify(sep),
    );
  }
});

test("bare AND/OR is reported as an error at the operator", (t) => {
  const result = toTsquery("apple AND");
  t.false(result.ok);
  const [error] = result.diagnostics.items;
  t.is(error?.code, "bare-operator");
  t.is(error?.range.start.offset, 6);
  t.is(error?.range.end.offset, 9);
  t.false(isValid("OR banana"));
});
