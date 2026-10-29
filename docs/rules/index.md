# Rules

Every rule has a code of the form `OD` and three digits, and a name. `--select`, `--ignore`, the
configuration file and suppression comments take the code, or for the first three a prefix of it
such as `OD3`.

The hundreds digit says what the rule reads. A code is never renumbered and never reused for
another rule: once released, `OD203` means the same check in every later version, so baselines,
suppressions and links keep working. A retired rule leaves a gap.

A rule marked as needing the index checks one module against the others: the models, fields,
methods, XML ids and views of every module it depends on. It is off unless `base` is among the
indexed modules, and a notice says so. The other rules read one file, or one module, at a time.

The series of a rule is the range of Odoo versions it applies to. Each module is checked against
the series its manifest version names, and a module whose series is unknown gets only the rules
that apply to every series.

## OD0: parsing

A file that does not parse. Nothing else is checked in it.

| Code | Name | Reports |
| --- | --- | --- |
| [OD000](OD000.md) | `syntax-error` | The file does not parse. |

## OD1: Python, one file at a time

Python code checked on its own: security, deprecated APIs and ORM mistakes visible in one file.

| Code | Name | Reports |
| --- | --- | --- |
| [OD101](OD101.md) | `sql-injection` | cr.execute() receives a query built by string formatting. |
| [OD102](OD102.md) | `translation-interpolated` | _() receives an already formatted string, so the source text cannot be translated. |
| [OD103](OD103.md) | `cursor-commit` | Business code commits the cursor, breaking the request's transaction. |
| [OD104](OD104.md) | `default-evaluated-at-import` | A field default is called once at import instead of on each record. |
| [OD105](OD105.md) | `missing-super` | An ORM override never calls super(). |
| [OD106](OD106.md) | `name-get-deprecated` | name_get() is ignored; override_compute_display_name(). |
| [OD107](OD107.md) | `api-multi-one-removed` | @api.multi and @api.one no longer exist. |
| [OD108](OD108.md) | `group-operator-renamed` | Field attribute group_operator is renamed aggregator. |
| [OD109](OD109.md) | `private-env-shortcut` | self._cr, self._uid and self._context are deprecated; use self.env. |
| [OD110](OD110.md) | `sql-constraints-unsupported` | _sql_constraints is ignored; declare models.Constraint attributes. |
| [OD111](OD111.md) | `public-route-sudo` | A public or auth='none' route escalates with sudo(). |
| [OD112](OD112.md) | `csrf-disabled` | An HTTP route disables CSRF protection. |
| [OD113](OD113.md) | `fields-view-get-removed` | fields_view_get() is removed; override _get_view() or get_views(). |
| [OD114](OD114.md) | `depends-on-id` | @api.depends names the field id, which Odoo refuses. |
| [OD115](OD115.md) | `dotted-onchange-constrains` | @api.onchange or @api.constrains names a dotted path, which Odoo ignores. |
| [OD116](OD116.md) | `field-shadows-model-member` | A field is named like a method or an attribute every model has. |
| [OD117](OD117.md) | `create-not-batched` | create() is decorated with @api.model, so Odoo calls it once per record. |
| [OD118](OD118.md) | `create-reads-dict` | create() under @api.model reads its argument as one dict, but receives a list of them. |
| [OD119](OD119.md) | `override-drops-result` | A create(), write() or copy() override calls super() but returns nothing. |
| [OD120](OD120.md) | `ormcache-unknown-name` | An ormcache key names something that is not a parameter of the method. |

## OD2: models and fields

Models, fields and methods checked against every module in the dependency graph.

| Code | Name | Reports |
| --- | --- | --- |
| [OD201](OD201.md) | `unknown-model` | The model is not defined by any indexed module. |
| [OD202](OD202.md) | `model-not-in-depends` | The model comes from a module missing from depends. |
| [OD203](OD203.md) | `unresolved-field-path` | A field path does not resolve on its model. |
| [OD204](OD204.md) | `missing-method` | compute, inverse, search, selection or group_expand names a method the model does not have. |
| [OD205](OD205.md) | `field-type-changed` | An inherited field is redefined with another type. |
| [OD206](OD206.md) | `unknown-env-model` | env[...] names a model no indexed module defines. |
| [OD207](OD207.md) | `related-type-mismatch` | A related field has another type than the field it relates to. |
| [OD208](OD208.md) | `invalid-one2many-inverse` | A One2many's inverse field is not a Many2one. |
| [OD209](OD209.md) | `model-redefined` | A new _name replaces a model that a module in depends already defines. |
| [OD210](OD210.md) | `unknown-domain-field` | A literal domain or group_by names a field its model does not have. |
| [OD211](OD211.md) | `orphan-super` | super() calls a method no parent class defines. |
| [OD212](OD212.md) | `monetary-without-currency` | A Monetary field's currency field is not a field of its model. |
| [OD213](OD213.md) | `required-many2one-set-null` | A required Many2one declares ondelete='set null'. |
| [OD214](OD214.md) | `many2many-table-clash` | Two Many2many fields of a model to one comodel get the same table. |
| [OD215](OD215.md) | `many2many-table-too-long` | A Many2many's relation table name is longer than PostgreSQL allows. |
| [OD216](OD216.md) | `selection-add-without-ondelete` | selection_add leaves a value of a required Selection with ondelete 'set null'. |
| [OD217](OD217.md) | `parent-store-without-path` | A model sets _parent_store without an indexed parent_path field. |
| [OD218](OD218.md) | `delegation-field-not-required` | A field that delegates to a parent model is not required with ondelete cascade or restrict. |
| [OD219](OD219.md) | `tracking-without-mail-thread` | A field sets tracking on a model that does not inherit mail.thread, which Odoo ignores. |
| [OD220](OD220.md) | `duplicate-field-label` | Two fields of a model have the same label. |
| [OD221](OD221.md) | `self-dependency-not-recursive` | A computed field depends on itself through other records without recursive=True. |
| [OD222](OD222.md) | `check-company-without-company` | A field sets check_company to a comodel without company_id. |
| [OD223](OD223.md) | `compute-leaves-field-unassigned` | A compute method leaves a non-stored readonly field unassigned on some records. |

## OD3: XML ids, views and data

XML ids, views and data records, within a file and across the dependency graph.

| Code | Name | Reports |
| --- | --- | --- |
| [OD301](OD301.md) | `unresolved-xmlid` | The XML id is not defined by its module. |
| [OD302](OD302.md) | `xmlid-not-in-depends` | The XML id comes from a module missing from depends. |
| [OD303](OD303.md) | `duplicate-xmlid` | The same XML id is defined twice in one file. |
| [OD304](OD304.md) | `unknown-view-field` | A view references a field its model does not have. |
| [OD305](OD305.md) | `attrs-states-removed` | attrs and states are removed; use invisible, readonly and required expressions. |
| [OD306](OD306.md) | `tree-renamed-list` | The tree view type is renamed list. |
| [OD307](OD307.md) | `t-raw-unsafe` | t-raw is deprecated and renders unescaped HTML; use t-out with Markup. |
| [OD308](OD308.md) | `xmlid-used-before-definition` | An XML id is referenced before the file that defines it is loaded. |
| [OD309](OD309.md) | `unknown-record-field` | A data record sets a field its model does not have. |
| [OD310](OD310.md) | `unmatched-view-locator` | An inherited view locates an element its parent views do not have. |

## OD4: access

Access rights: models no one can reach, and rights granted too widely.

| Code | Name | Reports |
| --- | --- | --- |
| [OD401](OD401.md) | `model-without-access` | A new model has no ir.model.access rule. |
| [OD402](OD402.md) | `public-write-access` | Public or portal users get write, create or unlink access. |

## OD5: manifest and module graph

The manifest and the dependency graph: listed files, unlisted files and dependencies.

| Code | Name | Reports |
| --- | --- | --- |
| [OD501](OD501.md) | `manifest-data-missing` | A file listed in the manifest does not exist. |
| [OD502](OD502.md) | `data-file-not-loaded` | A data file is not listed in the manifest, so Odoo never loads it. |
| [OD503](OD503.md) | `unknown-dependency` | A module in depends is not on the addons path. |
| [OD504](OD504.md) | `circular-dependency` | A module in depends depends back on this module. |

## OD6: JavaScript and Owl

Scripts under `static/src` that a bundle loads, checked against the scripts of every indexed
module.

| Code | Name | Reports |
| --- | --- | --- |
| [OD601](OD601.md) | `unresolved-js-import` | A JS import names a module or an export no indexed module provides. |
| [OD602](OD602.md) | `orphan-patch-super` | A patch calls a super method the class it patches does not have. |
| [OD603](OD603.md) | `unknown-rpc-model` | An ORM call from JS names a model no indexed module defines. |
| [OD604](OD604.md) | `unknown-rpc-method` | orm.call() names a method its model does not have. |
| [OD605](OD605.md) | `unknown-template` | A component or a t-inherit names a template no indexed bundle defines. |
| [OD606](OD606.md) | `unmatched-template-locator` | A template locates an element its parent templates do not have. |
| [OD607](OD607.md) | `private-rpc-method` | orm.call() names a private method, which Odoo refuses to call over RPC. |

## OD7: data flows

Values a user controls, followed through one function to the calls that must not receive them.

The sources are the parameters of a controller method behind `@route`; the parameters of a public
model method (no leading `_`, no `@api.private`), which any user with access to the model can call
over RPC; and the request: `request.params`, `request.get_json_data()`,
`request.get_http_params()`, and the `args`, `form`, `values`, `files`, `cookies`, `headers`, `data`
and JSON of `request.httprequest`.

A value stays tainted through assignments, formatting, concatenation, f-strings, string methods,
items, attributes, collections, comprehensions and module functions such as `json.loads()`. Records
do not carry it: what a recordset method such as `browse()` or `search()` returns is not tainted.
Both branches of an `if` are followed, a branch that returns or raises adds nothing after it, and
a loop body is read twice.

A value passed to a method of `self`, of `env["x"]` or of a local bound to one is followed into
every definition of that method the module's dependencies hold, overrides included, up to four
calls deep; so is a value such a method returns. The finding is at the sink when the module holds
it, otherwise at the call that leaves the module. A method that runs under `sudo()` only because
its caller called it so is not known to write as superuser.

The flow ends at a sanitizer: `int()`, `float()`, `bool()`, `len()`, `SQL.identifier()`,
`escape()`, `html_escape()`, `html_sanitize()` and `plaintext2html()`; at `assert x in (...)` or
`if x not in allowed: raise`; and at an intersection with values the code chose, such as
`set(vals) & allowed`.

| Code | Name | Reports |
| --- | --- | --- |
| [OD701](OD701.md) | `sql-injection-flow` | Data a user controls reaches the text of an SQL query. |
| [OD702](OD702.md) | `code-injection-flow` | Data a user controls reaches safe_eval(), eval() or exec(). |
| [OD703](OD703.md) | `markup-injection-flow` | Data a user controls is wrapped in Markup() without escaping. |
| [OD704](OD704.md) | `sudo-mass-assignment` | A mapping a user controls is written as superuser, so the user picks the fields. |
