# Configuration

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
Excluded files are still indexed: they only go unreported. An `addons-path` entry may start with
`~`, and `oscn` exits with status 2 when one is not a directory.

## Taint table

The `[taint]` table (`[tool.odooscan.taint]` in `pyproject.toml`) adds sources, sinks and
sanitizers to those the [OD7 rules](rules/index.md#od7-data-flows) follow. No flag sets it.

```toml
[taint]
sources = ["portal_tools.read_form"]
sanitizers = ["portal_tools.clean_term", "sandbox.safe_eval"]

[taint.sinks]
OD701 = ["portal_tools.run_query"]
code-injection-flow = ["sandbox.run"]
```

A pattern is a dotted name. It matches a call or an attribute whose dotted path ends with the
pattern's last name and holds its other names in order, so `cr.execute` matches
`self.env.cr.execute` and `sudo().write` matches `env["x"].sudo().browse(ids).write`. A name other
than the last may end in `()` for a call, and `[]` stands for an item, as in `env.[].create`.

Configured rows match before the built-in ones, so the sanitizer `sandbox.safe_eval` silences
`sandbox.safe_eval(x)` while `safe_eval(x)` still reports OD702. Among themselves, sources match
first, then sanitizers, then sinks.

A source's value is read as a mapping whose keys the user picks too, so it can reach OD704 as well
as the other rules. Each key of `sinks` is one of the OD7 rules, by code or name, but OD716, which
reads what a route returns, and OD717, which reads the values of an attachment. A sink checks its
first positional argument, or without one the keyword `query`, `code`, `expr`, `vals` or
`vals_list`. An OD704 sink reports only a mapping whose keys the
user picks, and `%` on what an OD703 sink returns escapes nothing, unlike `%` on `Markup()`. `oscn`
exits with status 2 on a pattern or a rule it cannot read.
