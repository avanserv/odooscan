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
Excluded files are still indexed: they only go unreported.
