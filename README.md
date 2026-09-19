# @alidade/query-parser

Parser, linter and [TINQL](https://planetscale.com/docs/postgres/search/tinql)
emitter for Alidade boolean search queries. Rust core (no dependencies beyond
serde), shipped as a napi native binding for Node and a wasm build for the
browser, so the editor, the API and the query builder all run the same code.

```ts
import { toTinql, validate, format } from "@alidade/query-parser";

toTinql('"health care" AND policy NOT (medicare OR medicaid)');
// { ok: true, tinql: '("health care" AND policy) AND NOT (medicare OR medicaid)', diagnostics: { items: [] } }

validate("apple banana OR cherry").items[0].code; // "mixed-and-or"
format("apple -banana"); // "apple AND NOT banana"
```

Every function runs the same pipeline (lex → parse → lint → emit), so a query
`validate` accepts is guaranteed to transpile, and vice versa.

## Query language

| Syntax                           | Meaning                                                                                                            |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `climate policy`                 | Both terms (a space means AND; warned as `implicit-operator`)                                                      |
| `a AND b`, `a OR b`              | Boolean operators, UPPER CASE only; `and`/`or` are ordinary words                                                  |
| `NOT a`, `-a`, `a NOT b`         | Exclusion; emitted as TINQL `AND NOT`                                                                              |
| `(a OR b) AND c`                 | Grouping. AND binds tighter than OR, but mixing them at one level without parentheses is an error (`mixed-and-or`) |
| `"health care"`, `'health care'` | Exact phrase (adjacent, in order)                                                                                  |
| `"health care"~3`                | Phrase with up to 3 extra words between its terms, any phrase length; order-sensitive                              |
| `appl*`, `p?ach`                 | Wildcards: `*` = any characters, `?` = one character. `\*` / `\?` for the literal characters                       |
| `apple~1`, `apple~0:2`           | Fuzzy match within an edit distance (default max 2), optional fixed prefix length                                  |
| `a NEAR/3 b`                     | Both within 3 extra words of each other, either order                                                              |
| `a THEN/3 b`                     | `a` followed by `b` within 3 extra words                                                                           |
| `"AND"`, `"TO"`                  | Quote an UPPER CASE keyword to search for the word                                                                 |

Matching is case- and accent-insensitive with no stemming; punctuation splits
words the way TIN's tokenizer does (`covid-19` is the phrase `covid 19`,
`#tag` is `tag`, `nytimes.com` is one word). `^N` boosts and legacy
`content:` / `content_exact:` field prefixes are parsed and ignored with a
warning. Square brackets, ranges and match-all `*` are rejected.

## API

- `toTinql(query, options?)` → `{ ok, tinql?, diagnostics }`. `tinql` is one
  string for `content ==> $1`, with explicit parentheses everywhere, negation
  as `AND NOT`, and `* AND NOT (…)` for negation-only queries.
- `parse(query, options?)` → `{ ok, ast?, diagnostics, stats? }`. The AST is
  JSON; every node carries a byte `span`.
- `validate(query, options?)` → `DiagnosticList`; `isValid(query, options?)`.
- `format(query, options?)` → canonical text with explicit operators, or null.
- `getStats(query, options?)` → term count, depth, feature flags.

Options: `conjunctionMode` (default true; false makes a bare space mean OR),
`maxSlop` (default 20), `maxFuzzyDistance` (default 2).

Diagnostics carry `severity` (8 error, 4 warning, 2 info, 1 hint), a stable
`code`, and a `range` whose `line`/`column` are in UTF-16 units for editors and
whose `offset` is a UTF-8 byte offset. When the repair is unambiguous the
diagnostic also carries a `fix` (`{ title, range, replacement }`) an editor can
apply directly: insert the missing operator, upper-case `and`, drop an ignored
boost or no-op `~N`, remove a field prefix, add a missing `)`.

### Diagnostic codes

Errors (query is rejected): `bare-operator`, `mixed-and-or`, `unbalanced-paren`,
`empty-group`, `unterminated-phrase`, `empty-phrase`, `empty-query`,
`unsupported-syntax` (`[…]`), `match-all` (`*`), `unsearchable-term` (no
letters or digits, would match nothing), `empty-wildcard`, `invalid-wildcard`,
`invalid-fuzzy`, `fuzzy-too-large`, `invalid-slop`, `slop-too-large`,
`invalid-proximity` (`NEAR` without `/N`), `negation-in-proximity`,
`invalid-boost`, `dangling-modifier`, `unexpected-token`.

Warnings: `implicit-operator`, `leading-wildcard`, `short-wildcard` (`a*`),
`wildcard-in-phrase`, `slop-no-effect`, `boost-ignored`, `field-ignored`.
Info: `literal-keyword`. Hint: `lowercase-operator` (`and` where `AND` was
probably meant).

## Development

```sh
cargo test                 # parser, linter and emitter unit tests
pnpm build:debug && pnpm test
TEST_WASI=1 pnpm test      # same suite on the wasm binding
pnpm test:tin              # + parity against a Lead/TIN Postgres, see scripts/lead/README.md
```

Publishing: `npm version <minor|patch>` on `main` (the bare-version commit
message gates the publish job), then `git push origin main --follow-tags`.
