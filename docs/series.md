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

[OD202](rules/OD202.md) and [OD603](rules/OD603.md) stay silent on a model only a module missing
from `depends` defines when its name fits what code outside `tests/` may build: the start an
expression shows, every name when it shows no literal start, and for a class of unknown series the
name the other reading gives it, unless that name is one of the class's parents.
