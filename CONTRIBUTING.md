# Contributing to Forge

## Local Workflow

Forge expects every change to keep the repository green with the same contract used in CI.

Recommended local flow:

```bash
make verify
```

Before release preparation:

```bash
make verify-release
```

## Verification Targets

- `make fmt-check`: formatting check
- `make test`: all targets, examples, and acceptance suites
- `make fixture-check`: explicit blueprint and plugin fixture checks
- `make clippy`: clippy with `-D warnings`
- `make package-check`: package dry-run
- `make verify`: format, tests, clippy, and fixture checks
- `make verify-release`: full verification plus package dry-run

## Contribution Expectations

- Keep the public API strongly typed. Do not reintroduce raw semantic strings where typed identifiers or enums already exist.
- Preserve the thin-app consumer model from the blueprint.
- Prefer updating examples, docs, and acceptance fixtures alongside public API changes.
- If you touch bootstrap or registry behavior, keep both fixture families green:
  - `tests/fixtures/blueprint_app`
  - `tests/fixtures/plugin_consumer_app`

## Documentation Expectations

- Keep the README quick-start aligned with the current typed API.
- Update `CHANGELOG.md` for user-visible behavior or workflow changes.
- Update the release checklist if the verification or publish flow changes.
