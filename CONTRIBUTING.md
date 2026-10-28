# Contributing

## Setup

You need rustup, `make`, `git` and [prek](https://github.com/j178/prek) (or pre-commit). rustup
installs the toolchain `rust-toolchain.toml` pins on the first cargo call.

```bash
make tools    # cargo-deny and cargo-machete, at the versions CI uses
make hooks    # installs the git hooks and runs them over the whole tree
git config commit.template .gitmessage
```

## The gates

`make check` runs everything CI runs but the git hooks: formatting (rustfmt), lints with warnings
as errors (clippy), unused dependencies (cargo-machete), advisories, licenses and sources
(cargo-deny, configured in `deny.toml`), and the tests. `make help` lists the individual targets.

The git hooks add what only makes sense there: secrets (gitleaks), spelling (typos), Markdown
(markdownlint), function complexity of at most 10 (polyscan) and the workflow audits (actionlint,
zizmor, schema and timeout checks).

## How the workspace is laid out

- `odooscan_core` holds the parsers, the index and every rule. It never prints, and never reads
  anything but the paths it is given.
- `odooscan` is the CLI, built as the `oscn` executable: flags, output formats and the exit status, and no rule logic.

A dependency is declared in the same change as the code that uses it, never ahead of it:
cargo-machete fails otherwise.

## Rules

A rule is registered in `crates/odooscan_core/src/rules.rs` with its code, series range, severity
and category, and raised from the file that owns its facts: `python.rs`, `xml.rs`, `csvdata.rs`,
`js.rs`, `manifest.rs`, or `checks.rs` when it needs the index.

Every rule comes with its page, `docs/rules/<code>.md`, and a row in `docs/rules/index.md`. A
test holds each page's heading and facts table to the registry. The page's failing example must
fire the rule and its fixed one must not: check both on a scratch module before opening the pull
request. A code is never renumbered or reused: a retired rule's code stays a gap.

Every rule fires at least once in `crates/odooscan_core/tests/fixtures/custom`, and a rule gated
by series also has a module of an earlier series where it stays silent. After changing what a rule
reports, run `cargo insta review` and read every changed line.

Fixtures are written by hand. Nothing is copied from Odoo, Odoo Enterprise or a customer module.

## The false-positive oracle

Odoo core is correct code, so its findings are recorded per series in `tests/oracle/`, and the CI
`oracle` job fails when a scan of it finds anything new or no longer finds something recorded. The
`Makefile` pins the odoo/odoo commit of each series. To run it, after any change to `index.rs` or
`checks.rs`:

```bash
make oracle-fetch oracle SERIES=18.0    # and SERIES=20.0
```

When a change is meant to alter what core reports, `make oracle-update SERIES=...` rewrites the
file. Read every changed entry and explain each new one in the pull request: an index-aware finding
on core is a false positive until shown otherwise. To bump a pinned commit, change it in the
`Makefile`, then fetch and update the same way.

## Commits and pull requests

Commits follow [Conventional Commits](https://www.conventionalcommits.org):
`<type>(<scope>): Summary`, with a source file or the CLI as the scope. `.gitmessage` has the
details.

Pull requests are squash-merged, so **the pull request title is the commit**. CI checks that it is
a conventional one.

- `feat` bumps the minor version and `fix` the patch; use them only for a change a user of the
  binary can see, such as a new rule or a finding that no longer fires.
- `feat!`, or a `BREAKING CHANGE:` footer, is a breaking change: a minor bump while the version is
  below 1.0, a major one after. Renumbering a rule code is one.
- `perf` and `revert` bump the patch too: whatever appears in the changelog is released.
- Everything else (`docs`, `chore`, `refactor`, `test`, `ci`, `build`, `style`) bumps nothing.

## Releases

Nobody edits a version. [Release Please](https://github.com/googleapis/release-please) reads the
commits on `main` and keeps a release pull request open with the next version and the changelog.

Before merging one, wait for the Release run that last updated it to finish and for CI to pass on
the branch head: Release Please does not know about `Cargo.lock`, so that run pushes a
`chore: sync Cargo.lock with the release version` commit whenever the version changed.

Merging the release pull request tags the release. The Release workflow then builds a wheel per
platform and the sdist, publishes them to PyPI through trusted publishing (environment `pypi`),
attaches them to the GitHub release with `SHA256SUMS`, and moves the pin and tag of
[odooscan-pre-commit](https://github.com/avanserv/odooscan-pre-commit). The CI Wheel job builds
the Linux wheel on every pull request, so a packaging break shows before release day.
