# Fixing findings

`oscn check --fix` rewrites the reported findings that have a mechanical fix in place, then
reports the findings left. `oscn check --diff` prints the same rewrites as a unified diff and
writes nothing.

```sh
oscn check path/to/module --addons-path ~/repos/odoo/odoo --diff > fixes.patch
patch -p1 < fixes.patch    # writes what --fix writes, with two exceptions
```

`patch` also writes a file that `--fix` finds it must leave alone only when it tries the write, as
one of another user, and under root it misses a read-only file, which `--diff` leaves out and
`--fix` writes. [What is fixed](#what-is-fixed) lists the skips.

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
| [OD306](rules/OD306.md) | `<tree>` becomes `<list>`, in its opening and closing tags | an xpath of a view in its inheritance tree names `tree`, as a step, or as a string beside a call to `name()` or `local-name()`; a module that is not checked has a `<tree>` there; a `<tree>` spec and a `<tree>` it may find are both there; the name has a prefix, or comes from a DOCTYPE entity |
| [OD306](rules/OD306.md) | each `tree` of a `view_mode` becomes `list` | the `view_mode` also lists `list`; the field has `eval`, `ref`, `search`, `file` or `type`; its text holds an entity, a CDATA section or a comment |
| [OD325](rules/OD325.md) | `t-esc` becomes `t-out` | the element also has `t-out`, `t-field` or `t-raw`; a view in its inheritance tree locates an element by its `t-esc`, through a spec or an xpath, or with `<attribute>` sets `t-esc` or `t-field`, or adds to, removes from or empties `t-out`; the attribute has a prefix, or comes from a DOCTYPE entity |

Removing the last `@api.multi` of a file can leave `api` imported and unused; the import stays.
`tracking=True` on a model that does not inherit `mail.thread` is then reported by
[OD219](rules/OD219.md), which `track_visibility=` needed just as much.

A rename in a view is left alone while another view in its inheritance tree finds the element by
its old name, sets `t-esc` or `t-field` on it, or adds to, removes from or empties its `t-out`:
the first would stop the install, `t-out` would hide the value of the second, and the third would
join the renamed value or delete it. A `<tree>` spec and a `<tree>` it may find both stay, even in
checked modules: `--exclude`, the baseline or a file left unwritten could fix one without the
other. Only the views of the scan are known: a module installed beside it that extends one of its
views by `tree` or `t-esc` breaks once the view is fixed, so pass such modules with
`--addons-path`.

## What is fixed

Only what the check reports: a finding hidden by `--select`, `--ignore`, `--exclude`, a
[suppression comment](suppressing.md) or the [baseline](baseline.md) stays as it is. A module is
fixed for the series it is checked as, so under `--target-series 18.0` a 17.0 module gets the
rewrites of 18.0, as [Series](series.md) describes. A Python file that does not parse is never
fixed, and neither is an XML file whose text is not its bytes, as one in Latin-1 with a character
above 127: an ASCII file that declares Latin-1 is fixed.

Each file is written once, with every fix of its findings. The bytes around a fix stay as they
are, so the formatting, a byte order mark and CRLF line ends survive, and so do the mode and the
group of the file. When two fixes would touch the same text, the first is applied and the other
finding stays reported. A second `--fix` changes nothing the first one fixed.

A file is left alone with a notice when its text changed between the check reading it and the
fix, or when putting the rewrite in its place would change more than its text: when the user cannot
write it, when it has other hard links, when it belongs to another user, or when it belongs to a
group its owner is not in. The last three are checked on Unix only.

`--diff` gives the same notice and leaves the file out of the diff when its text changed, which
both find by comparing it with what the check read, and when its mode lets no one write it or it
has other hard links, which its metadata tells without opening it for write. Other skips and
failures show only on a real write, so `--diff` still prints the rewrite of, for example, a file of
another user or of a group its owner is not in, one that its mode lets others write but not the
user running it, one that an ACL or a read-only mount protects, or one in a directory where no file
can be created. Under root, it leaves out a read-only file that `--fix` writes.

## Exit status and flags

With `--fix`, the exit status is 1 when findings are left, those of a file left alone included, 0
when none are, and 2 when a write fails; the other files are still written. With `--diff` it is 1
when anything is reported, whether it has a fix or not, so a CI job running `--diff` fails until
the code is clean, and 2, as with `--fix`, when the metadata of a file to rewrite cannot be read.

`--fix` and `--diff` cannot be combined, and neither can be combined with `--write-baseline`.
`--diff` prints only the diff, so it also refuses `--format` and `--statistics`; `--fix` reports
what is left in any format.
