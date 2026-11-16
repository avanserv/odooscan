# Contributing

## Setup

You need rustup, `make`, `git` and [prek](https://github.com/j178/prek) (or pre-commit). rustup
installs the toolchain `rust-toolchain.toml` pins on the first cargo call.

```bash
make tools    # cargo-deny and cargo-machete at the versions CI uses, and cargo-insta
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
- `odooscan` is the CLI, built as the `oscn` executable: flags, output formats, the exit status,
  fetching the core of `--target-series` with git into the user cache, and rewriting files with the
  fixes of `--fix` (`fixes.rs`). It holds no rule logic.

A dependency is declared in the same change as the code that uses it, never ahead of it:
cargo-machete fails otherwise.

## Rules

A rule is registered in `crates/odooscan_core/src/rules.rs` with its code, series range, severity
and category, and raised from the file that owns its facts: `python.rs`, `xml.rs`, `csvdata.rs`,
`js.rs`, `owl.rs`, `manifest.rs`, `po.rs`, or `checks.rs` when it needs the index or the module's
series, from facts the parser leaves to it, as OD116, OD120 and OD121 do for the series.

Every rule comes with its page, `docs/rules/<code>.md`, and a row in `docs/rules/index.md`. A
test holds each page's heading and facts table to the registry. The page's failing example must
fire the rule and its fixed one must not: check both on a scratch module before opening the pull
request. A code is never renumbered or reused: a retired rule's code stays a gap.

Every rule fires at least once in `crates/odooscan_core/tests/fixtures/custom`, and a rule gated
by series also has a module of an earlier series where it stays silent.

After changing what a rule reports, accept the snapshot change and read every changed line before
the commit, whichever route you take. `cargo insta review`, from the cargo-insta that `make tools`
installs, is the interactive route. Without it, `INSTA_UPDATE=always` makes insta write each
changed `.snap` in place and pass the test, with or without `CI` set; git then shows the change:

```bash
INSTA_UPDATE=always cargo test --workspace --locked
git status --short -- '*.snap'
git diff -- '*.snap'
```

A new snapshot is an untracked file that `git diff` does not list, so read it from `git status`
too. Insta never deletes a snapshot: a renamed or removed test leaves its old `.snap`, which you
remove by hand.

The snapshot scans `custom/` against `core/`; a case that cannot join that scan gets its own root
beside them. A case that could send the index round a cycle goes under `cycles/`, never `custom/`,
with its test in `crates/odooscan/tests/cli.rs`. There `cycle_paths` spawns
`oscn check cycles/<module> --addons-path core` through `check_in`. A cycle the index fails to end
overflows the stack and aborts the process; in `custom/` it would take the snapshot test binary
down with every test in it. `bootstrap/` holds a module named `base`, scanned in process with no
addons path by `base_creates_none_of_its_ids`: in `custom/` it would shadow `core/base`, which
every custom module depends on, since a checked module wins over a same-named indexed one. It must
be called `base` because OD324 exempts base's bootstrap records by that name.

A rule with a fix attaches it where it raises the finding, guarded against every form the rewrite
could change the meaning of. The fix gets a row in the table of `docs/fixing.md`, a Fixing section
on the rule's page, and tests that rewrite each fixed form, re-analyze it clean, and leave each
guarded form unfixed.

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

A comment or doc that cites Odoo source cites a path, with or without a line, in an
`.oracle/<series>` checkout the `Makefile` pins. An odoo/odoo commit hash may date a series bound.
`crates/odooscan_core/tests/conventions.rs` fails `make check` on such a citation in a file under
`crates/` or `docs/` whose series the `Makefile` does not pin.

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
