import test from "ava";
import binding from "./binding.cjs";

const { parse, validate, isValid, format, getStats, toTinql } = binding;

const codes = (query: string) =>
  validate(query).items.map((d: { code?: string }) => d.code);

test("parse valid query", (t) => {
  const result = parse("apple OR orange");
  t.true(result.ok);
  t.is(result.diagnostics.items.length, 0);
  t.is(result.stats?.termCount, 2);
  const ast = JSON.parse(result.ast ?? "null");
  t.is(ast.type, "or");
  t.deepEqual(ast.children[1].span, { start: 9, end: 15 });
});

test("parse invalid query", (t) => {
  const result = parse("apple AND");
  t.false(result.ok);
  t.is(result.diagnostics.items[0]?.code, "bare-operator");
  t.is(result.diagnostics.items[0]?.message, "AND has no search term after it");
});

test("isValid", (t) => {
  t.true(isValid("foo AND bar"));
  t.false(isValid("(foo"));
  t.false(isValid("OR banana"));
  t.false(isValid("foo AND bar OR baz"));
});

test("format normalizes query", (t) => {
  t.is(format("foo bar"), "foo AND bar");
  t.is(format("foo -bar"), "foo AND NOT bar");
  t.is(format("(foo"), null);
});

test("validate returns diagnostics", (t) => {
  t.deepEqual(codes("foo AND bar"), []);
  t.deepEqual(codes("foo bar"), ["implicit-operator"]);
  t.deepEqual(codes("foo AND bar OR baz"), ["mixed-and-or"]);
});

test("getStats reports features", (t) => {
  const stats = getStats('"a b"~2 AND NOT c* AND d~1 AND e NEAR/3 f');
  t.true(stats?.hasPhrase);
  t.true(stats?.hasNegation);
  t.true(stats?.hasWildcard);
  t.true(stats?.hasFuzzy);
  t.true(stats?.hasProximity);
  t.is(stats?.termCount, 5);
});

test("toTinql emits explicit parentheses and AND NOT", (t) => {
  const result = toTinql('"health care" AND policy NOT (medicare OR medicaid)');
  t.true(result.ok);
  t.is(
    result.tinql,
    '("health care" AND policy) AND NOT (medicare OR medicaid)',
  );
});

test("toTinql rewrites negation-only queries with match-all", (t) => {
  t.is(toTinql("NOT spam").tinql, "* AND NOT spam");
  t.is(toTinql("apple OR NOT spam").tinql, "apple OR (* AND NOT spam)");
});

test("toTinql supports wildcards, fuzzy and proximity", (t) => {
  t.is(toTinql("appl* AND p?ach").tinql, "appl* AND p?ach");
  t.is(toTinql("apple~1").tinql, "apple~1");
  t.is(
    toTinql('(apple OR pear) NEAR/5 "hot pie"').tinql,
    '(apple OR pear) NEAR/5 "hot pie"',
  );
  t.is(toTinql('"one two three"~7').tinql, '"one two three"~7');
});

test("toTinql quotes reserved words and syntax characters", (t) => {
  t.is(toTinql("TO AND WITHIN").tinql, '"TO" AND "WITHIN"');
  t.is(toTinql("foo\\(bar\\)").tinql, '"foo(bar)"');
  t.is(toTinql('"snake_case"').tinql, '"snake\\_case"');
});

test("toTinql fails with diagnostics", (t) => {
  const result = toTinql("appl* AND *");
  t.false(result.ok);
  t.is(result.tinql, undefined);
  t.deepEqual(
    result.diagnostics.items.map((d: { code?: string }) => d.code),
    ["match-all"],
  );
});

test("options: disjunction mode and limits", (t) => {
  t.is(
    toTinql("apple banana", { conjunctionMode: false }).tinql,
    "apple OR banana",
  );
  t.false(toTinql('"a b"~30').ok);
  t.true(toTinql('"a b"~30', { maxSlop: 50 }).ok);
  t.false(toTinql("apple~3").ok);
  t.true(toTinql("apple~3", { maxFuzzyDistance: 3 }).ok);
});

test("operators followed by a newline, CRLF, or tab still bind", (t) => {
  for (const sep of ["\n", "\r\n", "\t"]) {
    const and = toTinql(`(test OR test)${sep}AND${sep}(testing OR testing)`);
    t.true(and.ok, JSON.stringify(sep));
    t.is(
      and.tinql,
      "(test OR test) AND (testing OR testing)",
      JSON.stringify(sep),
    );
    t.is(and.diagnostics.items.length, 0, JSON.stringify(sep));
  }
});

test("diagnostics carry editor positions", (t) => {
  const result = validate("apple AND\nbanana cherry");
  const [warning] = result.items;
  t.is(warning?.code, "implicit-operator");
  t.is(warning?.range.start.line, 1);
  t.is(warning?.range.start.column, 0);
  t.is(warning?.range.end.column, 13);

  const error = validate("日本 AND 😀x *").items.find(
    (d: { code?: string }) => d.code === "match-all",
  );
  t.truthy(error);
  t.is(error?.range.start.column, 11);
});
