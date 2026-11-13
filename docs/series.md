# Series

Each rule declares the Odoo series it applies to, and every module is checked against its own
series. The series is the first two parts of a five-part manifest version, so `18.0.1.2.0` means
18.0. Pass `--odoo-version 18.0` for modules whose version does not name one. Those modules
otherwise get only the rules that carry no range. `oscn rules --odoo-version 18.0` lists the
rules whose range does not exclude that series. A listed rule may still stay silent on it, as its
page says. [OD509](rules/OD509.md) alone reads the flag for every module: under it, a version that
names another series is reported, since that Odoo will not install it.

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
the core on the addons path, or the one fetched below when none is given, so a port shows what that
series removed or no longer defines. A module on the addons path keeps the series its own manifest
names, and one whose manifest names none takes the target. When an addons path entry is the root of
an Odoo checkout and its `odoo/release.py` names another series, a notice says so; an entry such as
the checkout's `odoo/addons` gives none, since the scan reads nothing outside the paths it is given.
The flag conflicts with `--odoo-version` and has no configuration key.

A summary then counts the findings per kind and per rule. A deprecation rule reporting at error
severity counts as a removed API, below it as a deprecated API. The index rules OD201-OD207,
OD210-OD212, OD224, OD227, OD229, OD230, OD301, OD302, OD304, OD309-OD312, OD314, OD316, OD320,
OD322, OD324, OD503 and OD601-OD606 count as missing upstream: each reports a model, field, column,
method, XML id, template, view element, selection value or module that the code names and nothing
it depends on defines, an override or hook that no longer fits its parent, an inherited or related
field whose type it no longer matches, or a required field a data record leaves unset. Every other
finding counts as other. With `--format text` the summary follows the findings, and `--statistics`
prints it alone; with any other format it goes to stderr. `--fix` applies the rewrites of the
target's findings, as [Fixing findings](fixing.md) describes.

When no addons path entry is an Odoo checkout and no `base` module is found on the addons path or
among the checked paths, `oscn` fetches the target's core with `git`, which must be on `PATH`. It
asks odoo/odoo for the head of the series' branch on each run, `18.0` for 18.0 and `saas-18.1` for
saas~18.1, and fetches that commit unless the cache holds it, without tests, translations and
assets other than scripts and templates. Each commit is a directory of `core/` in the cache. The
branch keeps its newest commit and the one before it, so that a run still reading the one before
keeps it. A run that brings the cache up to the head then removes leftovers untouched for a day:
what an interrupted run left, and commit directories no branch records. A run that falls back to
the cache removes nothing. The cache is `$ODOOSCAN_CACHE_DIR`, else
`$XDG_CACHE_HOME/odooscan` for an absolute `XDG_CACHE_HOME`, `%LOCALAPPDATA%\odooscan` on Windows,
or `~/.cache/odooscan`. On Windows, a cache in a deep directory may need `git config --global
core.longpaths true`. `ODOOSCAN_ODOO_REMOTE` replaces `https://github.com/odoo/odoo`, for a mirror
or a local clone. An ssh remote never prompts, gives up on a host that does not answer within 30
seconds and drops a connection silent for about 45 seconds, unless `GIT_SSH_COMMAND`, `GIT_SSH` or
`core.sshCommand` says how to run ssh.

Resolving the head gives up after a minute, and the fetch and the checkout after 30 minutes each; a
step that runs out of time is killed with every process it started. A transfer also gives up when it
stays below 1 kB/s for 30 seconds over HTTP. When the remote cannot be reached, a transfer stalls or
runs out of time, the branch is missing, `git` cannot run or the cache cannot be written, a notice
says why and the commit last fetched for the branch is used. Without one, `oscn` exits with status 2
and asks for `--addons-path`.
