# Fixing findings

`oscn check --fix` rewrites the reported findings that have a mechanical fix in place, then
reports the findings left. `oscn check --diff` prints the same rewrites as a unified diff and
writes nothing.

```sh
oscn check path/to/module --addons-path ~/repos/odoo/odoo --diff > fixes.patch
patch -p1 < fixes.patch    # writes what --fix writes
```

The diff names each file as the check reports it, relative to the current directory, so `patch
-p1` applies it from that directory when every path checked is under it. A file outside it gets a
name that climbs out with `..`, as `a/../mods/x.py`, or an absolute one, and `patch` refuses both
as dangerous. Unlike `patch`, `--fix` then reports the findings left and sets the exit status by
them.

## What has a fix

| Rule | Rewrite | Left alone when |
| --- | --- | --- |
| [OD107](rules/OD107.md) | removes `@api.multi` with its line | anything else is on the line, a comment included; `@api.one` returned a list, so it is never removed |
| [OD108](rules/OD108.md) | `group_operator=` becomes `aggregator=` | the call also passes `aggregator=`, or unpacks a dict with `**` |
| [OD109](rules/OD109.md) | `self._cr`, `self._uid` and `self._context` become `self.env.cr`, `self.env.uid` and `self.env.context` | the attribute is assigned or deleted, or the code is not in a model class |
| [OD125](rules/OD125.md), [OD126](rules/OD126.md) | `sudo(user)` becomes `with_user(user)` | `sudo()` is given anything beside the user |
| [OD125](rules/OD125.md), [OD126](rules/OD126.md) | `track_visibility="onchange"` becomes `tracking=True` | the value is not a non-empty string, or the call also passes `tracking=` or `track_sequence=`, or unpacks a dict with `**` |

Removing the last `@api.multi` of a file can leave `api` imported and unused; the import stays.
`tracking=True` on a model that does not inherit `mail.thread` is then reported by
[OD219](rules/OD219.md), which `track_visibility=` needed just as much.

## What is fixed

Only what the check reports: a finding hidden by `--select`, `--ignore`, `--exclude`, a
[suppression comment](suppressing.md) or the [baseline](baseline.md) stays as it is. A module is
fixed for the series it is checked as, so under `--target-series 18.0` a 17.0 module gets the
rewrites of 18.0, as [Series](series.md) describes. A Python file that does not parse is never
fixed.

Each file is written once, with every fix of its findings. The bytes around a fix stay as they
are, so the formatting, a byte order mark and CRLF line ends survive. A file that changed between
the check reading it and the fix is left alone with a notice, and so is a read-only file, one with
other hard links or one of another user, since putting the rewrite in its place would change more
than its text. When two fixes would touch the same text, the first is applied and the other
finding stays reported. A second `--fix` changes nothing the first one fixed.

## Exit status and flags

With `--fix`, the exit status is 1 when findings are left, those of a file left alone included, 0
when none are, and 2 when a write fails; the other files are still written. With `--diff` it is 1
when anything is reported, whether it has a fix or not, so a CI job running `--diff` fails until
the code is clean.

`--fix` and `--diff` cannot be combined, and neither can be combined with `--write-baseline`.
`--diff` prints only the diff, so it also refuses `--format` and `--statistics`; `--fix` reports
what is left in any format.
