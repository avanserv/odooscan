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
uv tool install odooscan    # or: pip install odooscan
oscn check path/to/custom-addons --addons-path ~/repos/odoo/odoo,~/repos/odoo/enterprise
```

The wheels hold a prebuilt binary for Linux (glibc and musl), macOS and Windows; `cargo build
--release` builds `target/release/oscn` anywhere else.

The [documentation](https://avanserv.github.io/odooscan/) covers the options,
[configuration](https://avanserv.github.io/odooscan/configuration/),
[CI](https://avanserv.github.io/odooscan/ci/),
[series](https://avanserv.github.io/odooscan/series/),
[baselines](https://avanserv.github.io/odooscan/baseline/) and
[suppression](https://avanserv.github.io/odooscan/suppressing/), with a
[page per rule](https://avanserv.github.io/odooscan/rules/). `oscn explain OD203` prints a rule's
page in the terminal.

## Layout

- `crates/odooscan_core/src/project.rs` finds modules, parses every file in parallel, and keeps
  per-file facts.
  - `python.rs`, `xml.rs`, `csvdata.rs` and `manifest.rs` extract those facts and raise the
    single-file rules.
- `index.rs` merges the facts. It resolves fields through `_inherit`, `_inherits` and
  `delegate=True`, related comodels, view inheritance, and the dependency scope of each module.
- `checks.rs` holds the cross-file rules.
- `rules.rs` is the registry: code, series range, severity and category. `docs/rules/` has a page
  per rule.
- `crates/odooscan` is the CLI.

## Testing

`cargo test` runs a snapshot over `crates/odooscan_core/tests/fixtures`. That fixture is a small
core (`base`, `mail`) plus custom modules written to trigger every rule; review changes with
`cargo insta review`. The false-positive oracle is Odoo itself: a scan of core must report exactly
the findings recorded in `tests/oracle/`, as CONTRIBUTING.md describes.
