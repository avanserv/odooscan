# Series

Each rule declares the Odoo series it applies to, and every module is checked against its own
series. The series is the first two parts of a five-part manifest version, so `18.0.1.2.0` means
18.0. Pass `--odoo-version 18.0` for modules whose version does not name one. Those modules
otherwise get only the rules that carry no range. `oscn rules --odoo-version 18.0` lists the
rules for a series.

The series also decides the model of a class without `_name` whose `_inherit` is a list or tuple,
which saas~18.1 names after the class as [OD209](rules/OD209.md) describes. A module of unknown
series reads it as before saas~18.1, and where the two readings differ, the module's models count as
built at runtime, as with a `_name` given by an expression: a model or model id that no indexed
module defines is not reported, and [OD604](rules/OD604.md) checks no method the module's scripts
call.
