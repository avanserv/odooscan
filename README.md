# odooscan

[![CI](https://github.com/avanserv/odooscan/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/avanserv/odooscan/actions/workflows/ci.yml?query=branch%3Amain)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![prek](https://img.shields.io/badge/prek-enabled-brightgreen?logo=pre-commit)](.pre-commit-config.yaml)
[![Complexity: polyscan](https://img.shields.io/badge/complexity-polyscan-brightgreen)](https://github.com/ludo-technologies/polyscan)
[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-fe5196?logo=conventionalcommits&logoColor=white)](https://www.conventionalcommits.org)

A quality and security scanner for Odoo modules, written in Rust. It parses Python with ruff's
parser, and XML and CSV natively. It then builds one index of every model, field, method, XML id,
view and access rule across the modules it is given, so rules can check what a single-file linter
cannot. For example, it can tell you that a view names a field its model lacks, or that an XML id
belongs to a module missing from `depends`.

```sh
cargo build --release
target/release/oscn check path/to/custom-addons --addons-path ~/repos/odoo/odoo,~/repos/odoo/enterprise
```

The command is `oscn`. `check` reports on the modules found under the given paths. `--addons-path` holds what those
modules build on. It is indexed but never reported on, and an Odoo source checkout stands for its
`addons` and `odoo/addons` directories. The variable `ODOOSCAN_ADDONS_PATH` sets the same thing.
Without `base` on the addons path, the rules marked `needs_index` in `src/rules.rs` are off and a
notice says so.

Other options:

- `--format json` and `--format sarif` are for CI and code review.
- `--select OD3` and `--ignore OD111` filter by code prefix.
- `--exclude migrations/,tests/` hides findings in files matching gitignore-style patterns.
- `--statistics` prints counts per rule.

The exit status is 1 when anything is reported, and 2 when the settings are invalid.

## Configuration

`oscn check` reads the nearest `odooscan.toml`, or `pyproject.toml` with a `[tool.odooscan]`
table, in the current directory or above it; `--config` names one instead. Its keys are the
`check` flags, and a flag or `ODOOSCAN_ADDONS_PATH` replaces the key of the same name.

```toml
[tool.odooscan]
addons-path = ["../odoo", "../enterprise"]
odoo-version = "18.0"
ignore = ["OD111"]
exclude = ["/legacy", "migrations/"]
baseline = "odooscan-baseline.json"
```

Paths in the file are relative to it, and so are anchored `exclude` patterns (those with a slash
before the end, such as `/legacy`). An unanchored one, such as `migrations/`, matches at any depth.
Excluded files are still indexed: they only go unreported.

## Series

Each rule declares the Odoo series it applies to, and every module is checked against its own
series. The series is the first two parts of a five-part manifest version, so `18.0.1.2.0` means
18.0. Pass `--odoo-version 18.0` for modules whose version does not name one. Those modules
otherwise get only the rules that carry no range. `oscn rules --odoo-version 18.0` lists the
rules for a series.

## Adopting on an existing codebase

`oscn check --write-baseline` records every current finding in `odooscan-baseline.json` beside the
configuration file, and later runs given `--baseline odooscan-baseline.json` (or the `baseline`
key) report only new ones. A finding is recorded by file, code and message, not line, so edits
around it do not bring it back. When findings are fixed, a notice says so; write the baseline again
to drop them and keep the file shrinking.

## Suppressing a finding

Write `odooscan: ignore[OD103]` in a comment, or `noqa: OD103`. A trailing comment covers its own
line, and a comment alone on a line covers the next one. `odooscan: ignore` without codes covers
every rule. XML takes the same text inside `<!-- -->`.

## Layout

- `crates/odooscan_core/src/project.rs` finds modules, parses every file in parallel, and keeps
  per-file facts.
  - `python.rs`, `xml.rs`, `csvdata.rs` and `manifest.rs` extract those facts and raise the
    single-file rules.
- `index.rs` merges the facts. It resolves fields through `_inherit`, `_inherits` and
  `delegate=True`, related comodels, view inheritance, and the dependency scope of each module.
- `checks.rs` holds the cross-file rules.
- `rules.rs` is the registry: code, series range, severity and category.
- `crates/odooscan` is the CLI.

## Testing

`cargo test` runs a snapshot over `crates/odooscan_core/tests/fixtures`. That fixture is a small
core (`base`, `mail`) plus custom modules written to trigger every rule; review changes with
`cargo insta review`. The false-positive oracle is Odoo itself. A scan of Odoo core must stay
near-silent for the index-aware rules, so run one after any change to them:

```sh
git -C ~/repos/odoo/odoo archive origin/18.0 -- addons odoo/addons ':(exclude,glob)**/static/**' | tar -x -C /tmp/odoo-18.0
oscn check /tmp/odoo-18.0 --odoo-version 18.0 --statistics
```
