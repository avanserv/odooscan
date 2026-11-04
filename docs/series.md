# Series

Each rule declares the Odoo series it applies to, and every module is checked against its own
series. The series is the first two parts of a five-part manifest version, so `18.0.1.2.0` means
18.0. Pass `--odoo-version 18.0` for modules whose version does not name one. Those modules
otherwise get only the rules that carry no range. `oscn rules --odoo-version 18.0` lists the
rules whose range does not exclude that series. A listed rule may still stay silent on it, as its
page says.

The series also decides the model of a class without `_name` whose `_inherit` is a list or tuple,
which saas~18.1 names after the class as [OD209](rules/OD209.md) describes. A module of unknown
series reads it as before saas~18.1, and where the two readings differ, the module's models count as
built at runtime, as with a `_name` given by an expression: a model id that no indexed module
defines is not reported, [OD201](rules/OD201.md) skips a model no indexed module defines, and
[OD604](rules/OD604.md) checks no method the module's scripts call.

[OD202](rules/OD202.md) and [OD603](rules/OD603.md) stay silent on a model only a module missing
from `depends` defines, and [OD206](rules/OD206.md) on a model no indexed module defines, when its
name fits what code outside `tests/` may build: the start an expression shows, every name when it
shows no literal start, and for a class of unknown series the name the other reading gives it,
unless that name is one of the class's parents.

`--target-series 19.0` checks every module given to `check` as if its manifest named 19.0, against
the core on the addons path, so a port shows what that series removed or no longer defines. A
module on the addons path keeps the series its own manifest names, and one whose manifest names none
takes the target. When an addons path entry is the root of an Odoo checkout and its
`odoo/release.py` names another series, a notice says so; an entry such as the checkout's
`odoo/addons` gives none, since the scan reads nothing outside the paths it is given. The flag
conflicts with `--odoo-version` and has no configuration key.

A summary then counts the findings per kind and per rule. A deprecation rule reporting at error
severity counts as a removed API, below it as a deprecated API. The index rules OD201-OD204, OD206,
OD210-OD212, OD224, OD227, OD229, OD230, OD301, OD302, OD304, OD309-OD312, OD314, OD316, OD320,
OD322, OD503 and OD601-OD606 count as missing upstream: each reports a model, field, column, method,
XML id, template, view element, selection value or module that the code names and nothing it
depends on defines, or an override or hook that no longer fits its parent. Every other finding
counts as other. With `--format text` the summary follows the
findings, and `--statistics` prints it alone; with any other format it goes to stderr.
