# odooscan

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

The command is `oscn`. `check` reports on the modules found under the given paths. `--addons-path` holds what those
modules build on. It is indexed but never reported on, and an Odoo source checkout stands for its
`addons` and `odoo/addons` directories. The variable `ODOOSCAN_ADDONS_PATH` sets the same thing.
Without `base` on the addons path, the [rules](rules/index.md) that need the index are off and a
notice says so.

Other options:

- `--format github` and `--format gitlab` are for [CI](ci.md); `json` and `sarif` for other
  tools.
- `--select OD3` and `--ignore OD111` filter by code prefix.
- `--exclude migrations/,tests/` hides findings in files matching gitignore-style patterns.
- `--statistics` prints counts per rule.

The exit status is 1 when anything is reported, and 2 when the settings are invalid.

`oscn explain OD203`, or `oscn explain unresolved-field-path`, prints a rule's page: what it
reports, why Odoo fails on it, and an example of the fix. [Rules](rules/index.md) lists every one.
