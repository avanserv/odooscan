# Adopting on an existing codebase

`oscn check --write-baseline` records every current finding in `odooscan-baseline.json` beside the
configuration file, and later runs given `--baseline odooscan-baseline.json` (or the `baseline`
key) report only new ones. A finding is recorded by file, code and message, not line, so edits
around it do not bring it back. When findings are fixed, a notice says so; write the baseline again
to drop them and keep the file shrinking.
