# odooscan

A Rust scanner for Odoo modules that indexes every model, field, XML id and view across the
modules it is given, so its rules can check what a single-file linter cannot. README.md has the
usage, and CONTRIBUTING.md has the layout, the gates and the release flow.

## Rules for changes

- Run `make check` before declaring a change done.
- After a change to `index.rs` or `checks.rs`, scan Odoo core 18.0 and the newest series as
  CONTRIBUTING.md describes, and account for every new finding before declaring it done.
- Every rule fires in the fixture snapshot. Accept a snapshot change only after reading each
  changed line.
- Never copy code from pylint-odoo (AGPL) or Odoo (LGPL) into this MIT repository: reimplement
  from the behaviour.
- Never put Odoo, Odoo Enterprise or customer code in a fixture.
- Never edit a version or `CHANGELOG.md`. Release Please owns both.
- Pin every GitHub Action to a commit SHA with the tag in a trailing comment. `.github/zizmor.yml`
  enforces it.
