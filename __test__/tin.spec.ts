// Runs only when TIN_PSQL points at a Postgres with the `tin` extension (TIN on
// PlanetScale or Lead locally), e.g. TIN_PSQL="psql -h 127.0.0.1 -p 5499 -U lead -d lead".
// Every emitted TINQL string must parse there and match exactly the expected rows.
import test from "ava";
import { execFileSync } from "node:child_process";
import binding from "./binding.cjs";

const { toTinql } = binding;
const psql = process.env.TIN_PSQL?.split(" ").filter(Boolean);

const CORPUS = [
  [1, "Fuji apple pie with a flaky crust"],
  [2, "apple fuji pie"],
  [3, "The climate policy debate: climate change action now"],
  [4, "Health care reform and Medicare"],
  [5, "healthcare costs rising"],
  [6, "Wi-Fi outage at the office, e-mail down too"],
  [7, "Read it on nytimes.com today"],
  [8, "covid-19 vaccines and COVID19 boosters"],
  [9, "Follow @POTUS and #vote"],
  [10, "Rock and roll will never die"],
  [11, "peel and core only"],
  [12, "Nothing relevant here at all"],
  [13, "applesauce apples apply"],
  [14, "big bad wolf"],
  [15, "big very bad wolf"],
  [16, "o'brien said hello"],
  [17, "Jalapeño poppers"],
  [18, "wolf big bad"],
] as const;

function sql(query: string): string {
  if (!psql) throw new Error("TIN_PSQL is not set");
  const [cmd, ...args] = psql;
  return execFileSync(
    cmd!,
    [...args, "-Atq", "-v", "ON_ERROR_STOP=1", "-c", query],
    {
      encoding: "utf8",
    },
  ).trim();
}

const q = (s: string) => `'${s.replaceAll("'", "''")}'`;

test.before(() => {
  if (!psql) return;
  sql(`create extension if not exists tin;
    drop table if exists qp_corpus;
    create table qp_corpus(id int primary key, body text);
    insert into qp_corpus values ${CORPUS.map(([id, body]) => `(${id}, ${q(body)})`).join(",")};
    create index qp_corpus_tin on qp_corpus using tin (body);`);
});

const cases: Array<[query: string, expected: number[]]> = [
  ["apple AND pie", [1, 2]],
  ["apple pie", [1, 2]],
  ['"apple pie"', [1]],
  ['"fuji apple"~2', [1]],
  ['"climate action"~2', [3]],
  ['"health care"', [4]],
  ["healthcare", [5]],
  ["apple OR wolf", [1, 2, 14, 15, 18]],
  ["(apple OR wolf) AND NOT pie", [14, 15, 18]],
  ["(apple OR wolf) NOT pie NOT big", []],
  ["wolf NOT (big OR bad)", []],
  ["NOT (apple OR wolf)", [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17]],
  [
    "apple OR NOT (a* OR b* OR c* OR d* OR e* OR f* OR g* OR h* OR i* OR j* OR k* OR l* OR m* OR n* OR o* OR p* OR q* OR r* OR s* OR t* OR u* OR v* OR w* OR x* OR y* OR z* OR 0* OR 1* OR 2* OR 3* OR 4* OR 5* OR 6* OR 7* OR 8* OR 9*)",
    [1, 2],
  ],
  ["wi-fi", [6]],
  ['"wi fi"', [6]],
  ["e-mail", [6]],
  ["nytimes.com", [7]],
  ["nytimes", []],
  ["covid-19", [8]],
  ["covid19", [8]],
  ["@POTUS", [9]],
  ["potus", [9]],
  ["#vote", [9]],
  ["rock and roll", [10]],
  ["rock AND roll", [10]],
  ['"rock and roll"', [10]],
  ['"AND"', [4, 8, 9, 10, 11]],
  ["TO", []],
  ["big NEAR/1 wolf", [14, 18]],
  ["big NEAR/2 wolf", [14, 15, 18]],
  ["big THEN/1 wolf", [14]],
  ["big THEN/2 wolf", [14, 15]],
  ["big THEN/0 bad", [14, 18]],
  ['"big bad"~1', [14, 15, 18]],
  ['"bad big"~2', []],
  ["(big OR wolf) NEAR/0 bad", [14, 15, 18]],
  ["appl*", [1, 2, 13]],
  ["app?e", [1, 2]],
  ["apple~1", [1, 2, 13]],
  ["o'brien", [16]],
  ['"o\'brien said"', [16]],
  ["jalapeno", [17]],
  ["Jalapeño AND poppers", [17]],
  ["content_exact:apple AND content:pie", [1, 2]],
  ["apple^2 AND pie", [1, 2]],
  ["(test OR apple)\nAND\n(pie OR testing)", [1, 2]],
];

for (const [query, expected] of cases) {
  test(`tin parity: ${JSON.stringify(query)}`, (t) => {
    if (!psql) {
      t.pass("TIN_PSQL not set");
      return;
    }
    const out = toTinql(query);
    t.true(out.ok, JSON.stringify(out.diagnostics.items));
    const canonical = sql(`select tin.ql_parse(${q(out.tinql!)})`);
    t.truthy(canonical);
    const rows = sql(
      `select coalesce(array_to_json(array_agg(id order by id)), '[]') from qp_corpus where body ==> ${q(out.tinql!)}`,
    );
    t.deepEqual(JSON.parse(rows), expected, `tinql: ${out.tinql}`);
  });
}
