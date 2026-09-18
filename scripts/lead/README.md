# Lead (open-source TIN) for parity tests

[Lead](https://github.com/planetscale/lead) is PlanetScale's slow-but-exact
drop-in for the TIN extension. `__test__/tin.spec.ts` runs every emitted TINQL
string against it and checks the matched rows.

```sh
podman build -t alidade-lead-pg18 -f scripts/lead/Containerfile scripts/lead   # ~15 min
podman run -d --name alidade-lead-pg -p 127.0.0.1:5499:5432 \
  -e POSTGRES_USER=lead -e POSTGRES_PASSWORD=lead -e POSTGRES_DB=lead alidade-lead-pg18
pnpm test:tin
```

Pass a PlanetScale connection instead to test against real TIN:
`TIN_PSQL="psql <planetscale url>" pnpm test`.
