# Configuration

`oscn check` reads the nearest `odooscan.toml`, or `pyproject.toml` with a `[tool.odooscan]`
table, in the current directory or above it; `--config` names one instead. Its keys but the
[`taint` table](#taint-table) are the `check` flags, and a flag or `ODOOSCAN_ADDONS_PATH` replaces
the key of the same name.

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

A pattern is a dotted name. It matches a dotted path that ends with the pattern's last name and
holds its other names in order, so `cr.execute` matches `self.env.cr.execute` and `sudo().write`
matches `env["x"].sudo().browse(ids).write`. A source matches a call or an attribute, but a
sanitizer or a sink acts only on a call: the sanitizer `record.safe_text` ends the flow of
`cr.execute(q % record.safe_text())`, not of `cr.execute(q % record.safe_text)`. A sanitizer or a
sink on an attribute still hides a built-in source there, so `sanitizers = ["httprequest.data"]`
leaves `request.httprequest.data` untainted.

`[]` stands for an item, as in `env.[].create`, and a name other than the last may end in `()` for
a call. A last `[]` matches only a call of an item, so the sink `handlers.[]` matches
`handlers[k](x)`, and a source `forms.data.[]` never matches the read `forms.data['x']`.

`sudo()` in a pattern stands for running as superuser, as the [OD7
rules](rules/index.md#od7-data-flows) read it, rather than for the call: a call that builds a
superuser environment adds it, and `with_user()` for another user removes it, as `sudo(False)` does
except from saas~12.5 on the records of the superuser. So `sudo().write` matches
`x.with_user(SUPERUSER_ID).write(vals)` but not `x.sudo().with_user(user).write(vals)`, and
`sudo().with_user().write` matches only a `with_user()` for the superuser.

Configured rows match before the built-in ones, so the sanitizer `sandbox.safe_eval` silences
`sandbox.safe_eval(x)` while `safe_eval(x)` still reports OD702. Among themselves, sources match
first, then sanitizers, then sinks.

A source's value is read as a mapping whose keys the user picks too, so it can reach OD704 as well
as the other rules. Each key of `sinks` is one of the OD7 rules, by code or name, but OD716, which
reads what a route returns, and OD717, which reads the values of an attachment. A sink checks its
first positional argument, or without one the keyword `query`, `code`, `expr`, `vals` or
`vals_list`. An OD704 sink reports only a mapping whose keys the user picks, and `%` on what an
OD703 sink returns escapes nothing, unlike `%` on `Markup()`. `oscn` exits with status 2 on a
pattern or a rule it cannot read.
