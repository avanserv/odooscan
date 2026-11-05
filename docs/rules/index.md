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

## Load failures

Rule pages say a failure stops the install: that holds for a file of `data`. From saas~11.5 a
failure while loading a file of `demo` stops neither the install nor the update: Odoo logs a
warning and installs the module without its demo data. That covers the load alone: an update
validates a view of that file again after it, so a view that no longer validates stops the
update. Before saas~11.5 any failure in a `demo` file stops them like a data file.

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
| [OD111](OD111.md) | `public-route-sudo` | A public or auth='none' route runs as superuser. |
| [OD112](OD112.md) | `csrf-disabled` | An HTTP route disables CSRF protection. |
| [OD113](OD113.md) | `fields-view-get-removed` | fields_view_get() is removed; override _get_view() or get_views(). |
| [OD114](OD114.md) | `depends-on-id` | @api.depends names the field id, which Odoo refuses. |
| [OD115](OD115.md) | `dotted-onchange-constrains` | @api.onchange or @api.constrains names a dotted path, which Odoo ignores. |
| [OD116](OD116.md) | `field-shadows-model-member` | A field is named like a method or an attribute every model has. |
| [OD117](OD117.md) | `create-not-batched` | create() is decorated with @api.model, so Odoo calls it once per record. |
| [OD118](OD118.md) | `create-reads-dict` | create() under @api.model reads its argument as one dict, but receives a list of them. |
| [OD119](OD119.md) | `override-drops-result` | A create(), write() or copy() override calls super() but returns nothing. |
| [OD120](OD120.md) | `ormcache-unknown-name` | An ormcache key names something that is not a parameter of the method. |
| [OD121](OD121.md) | `cursor-rollback` | Business code rolls back the cursor, discarding the caller's work with its own. |
| [OD122](OD122.md) | `sql-write-without-invalidation` | An SQL write runs without invalidating the cache, so the ORM keeps stale values. |
| [OD123](OD123.md) | `patch-at-import` | Code run at import patches Odoo, for every database the server runs. |
| [OD124](OD124.md) | `module-level-cache` | A module-level dict, list or set filled at runtime is shared by every database. |
| [OD125](OD125.md) | `python-api-deprecated` | The module uses a Python API its series deprecates or ignores. |
| [OD126](OD126.md) | `python-api-removed` | The module uses a Python API its series removed. |
| [OD127](OD127.md) | `get-route-writes` | An HTTP route that accepts GET writes records, which Odoo checks no CSRF token for. |
| [OD128](OD128.md) | `cors-any-origin` | A route sets cors='*', so a script on any site can read its responses. |
| [OD129](OD129.md) | `auth-none-user` | An auth='none' route reads the user or writes records, but runs with no user. |
| [OD130](OD130.md) | `session-user-switch` | Code switches the user a request or a session runs as. |
| [OD131](OD131.md) | `secret-compared-with-eq` | A token, secret or signature is compared with == rather than consteq(). |

## OD2: models and fields

Models, fields and methods checked against every module in the dependency graph. A model class has
`Model`, `TransientModel`, `AbstractModel`, their aliases `osv`, `osv_memory` and `osv_abstract` or,
from 20.0, `CachedModel` among its bases, and any other base is a plain Python base class, which the
rules whose page says so skip, since only Python knows its fields and methods. As each page says,
they also skip a model when a class of it, or of a model it inherits or, for a rule reading fields,
delegates to, has an `_inherit` element that is not a literal, such as a variable, since the scan
cannot tell which model it names.

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
| [OD209](OD209.md) | `model-redefined` | A class redefines a model that a module in depends already defines. |
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
| [OD224](OD224.md) | `sql-unknown-column` | A literal SQL query names a column its model's table does not have. |
| [OD225](OD225.md) | `sql-view-missing-column` | The SQL view of a model selects no column for one of its stored fields. |
| [OD226](OD226.md) | `sql-view-not-created` | A model sets _auto = False, and nothing creates its SQL view. |
| [OD227](OD227.md) | `sql-definition-unknown-column` | An SQL constraint or index names a column its model's table does not have. |
| [OD228](OD228.md) | `delegate-without-ondelete` | A delegate=True Many2one has no ondelete, so Odoo fails when it sets up the registry. |
| [OD229](OD229.md) | `override-signature-mismatch` | An override refuses a call that the method it overrides takes. |
| [OD230](OD230.md) | `orphan-hook-method` | A compute, inverse, search or onchange method of a model extension is named nowhere, so Odoo never runs it. |

## OD3: XML ids, views and data

XML ids, views and data records, within a file and across the dependency graph.

| Code | Name | Reports |
| --- | --- | --- |
| [OD301](OD301.md) | `unresolved-xmlid` | The XML id is not defined by its module. |
| [OD302](OD302.md) | `xmlid-not-in-depends` | The XML id comes from a module missing from depends. |
| [OD303](OD303.md) | `duplicate-xmlid` | The same XML id is defined twice in one file, with another model or overwriting a field. |
| [OD304](OD304.md) | `unknown-view-field` | A view references a field its model does not have. |
| [OD305](OD305.md) | `attrs-states-removed` | attrs and states are removed; use invisible, readonly and required expressions. |
| [OD306](OD306.md) | `tree-renamed-list` | The tree view type is renamed list. |
| [OD307](OD307.md) | `t-raw-unsafe` | t-raw is deprecated and renders unescaped HTML; use t-out with Markup. |
| [OD308](OD308.md) | `xmlid-used-before-definition` | An XML id is referenced before the file that defines it is loaded. |
| [OD309](OD309.md) | `unknown-record-field` | A data record sets a field its model does not have. |
| [OD310](OD310.md) | `unmatched-view-locator` | An inherited view locates an element its parent views do not have. |
| [OD311](OD311.md) | `unknown-qweb-template` | A QWeb view or a report names a template no module in scope defines. |
| [OD312](OD312.md) | `unknown-button-method` | A button calls a method its model does not have. |
| [OD313](OD313.md) | `private-button-method` | A button calls a private method, which Odoo refuses. |
| [OD314](OD314.md) | `unknown-function-method` | A `<function>` of a data file calls a method its model does not have. |
| [OD315](OD315.md) | `invalid-modifier` | A view modifier is not an expression Odoo reads. |
| [OD316](OD316.md) | `unknown-modifier-name` | A view modifier names a field its model does not have. |
| [OD317](OD317.md) | `modifier-field-not-in-view` | A view modifier reads a field the view does not show. |
| [OD318](OD318.md) | `view-mode-without-view` | A window action lists a view type its model has no view for. |
| [OD319](OD319.md) | `python-field-syntax-error` | The code of a server action or a cron, or the file name of a report, is not Python that Odoo compiles. |
| [OD320](OD320.md) | `unknown-server-code-method` | The code of a server action or a cron calls a method its model does not have. |
| [OD321](OD321.md) | `wrong-model-reference` | A data record gives a relational field a record of another model. |
| [OD322](OD322.md) | `unknown-selection-value` | A data record gives a Selection field a value it does not list. |
| [OD323](OD323.md) | `duplicate-xmlid-across-files` | A data file defines an XML id another file of the module already defines, with another model, or overwriting the fields both name. |
| [OD324](OD324.md) | `missing-required-field` | A data record is created without a required field that has no default. |
| [OD325](OD325.md) | `t-esc-removed` | The server no longer renders t-esc in a QWeb view; use t-out. |
| [OD326](OD326.md) | `kanban-box-renamed-card` | The kanban-box template of a kanban view is renamed card. |
| [OD327](OD327.md) | `chatter-div-replaced` | A form's div with class oe_chatter no longer shows the chatter; use `<chatter/>`. |
| [OD328](OD328.md) | `bundle-template-removed` | A data file inherits or references an asset bundle, which is no longer a template. |

## OD4: access

Access rights: models no one can reach, rights granted too widely, fields and records they expose
to public and portal users, and record rules Odoo refuses to load.

| Code | Name | Reports |
| --- | --- | --- |
| [OD401](OD401.md) | `model-without-access` | A new model has no ir.model.access rule, or from saas~19.4 no ir.access rule with a group. |
| [OD402](OD402.md) | `public-write-access` | An access row gives public or portal users write, create or unlink, through their group or no group. |
| [OD403](OD403.md) | `unsanitized-public-html` | An Html field that public or portal users can write is not sanitized. |
| [OD404](OD404.md) | `public-secret-field` | A field named as a secret has no groups, on a model public or portal users can read. |
| [OD405](OD405.md) | `public-rule-always-true` | A record rule or ir.access domain gives public or portal users every record of a model they can reach. |
| [OD406](OD406.md) | `rule-without-permission` | A record rule sets every perm_* false, which Odoo refuses as it loads it. |

## OD5: manifest and module graph

The manifest and the dependency graph: listed files, unlisted files and dependencies.

| Code | Name | Reports |
| --- | --- | --- |
| [OD501](OD501.md) | `manifest-data-missing` | A file listed in the manifest does not exist. |
| [OD502](OD502.md) | `data-file-not-loaded` | A data file is listed under no manifest key Odoo reads, so Odoo never loads it. |
| [OD503](OD503.md) | `unknown-dependency` | A module in depends is not on the addons path. |
| [OD504](OD504.md) | `circular-dependency` | A module in depends depends back on this module. |
| [OD505](OD505.md) | `qweb-key-ignored` | The manifest lists templates under qweb, which Odoo ignores. |
| [OD506](OD506.md) | `assets-qweb-unloaded` | The manifest adds files to web.assets_qweb, which Odoo no longer loads. |

## OD6: JavaScript and Owl

Scripts under `static/src` that a bundle loads, checked against the scripts of every indexed
module.

| Code | Name | Reports |
| --- | --- | --- |
| [OD601](OD601.md) | `unresolved-js-import` | A JS import names a module or an export no indexed module provides. |
| [OD602](OD602.md) | `orphan-patch-super` | A patch calls a super method the class it patches does not have. |
| [OD603](OD603.md) | `unknown-rpc-model` | An ORM call from JS names a model no indexed module defines. |
| [OD604](OD604.md) | `unknown-rpc-method` | orm.call() names a method its model does not have. |
| [OD605](OD605.md) | `unknown-template` | A component, a t-inherit or a t-call names a template no indexed bundle defines. |
| [OD606](OD606.md) | `unmatched-template-locator` | A template locates an element its parent templates do not have. |
| [OD607](OD607.md) | `private-rpc-method` | orm.call() names a private method, which Odoo refuses to call over RPC. |

## OD7: data flows

Values a user controls, followed through one function to the calls that must not receive them or,
for OD716, to what a route returns.

The sources are the parameters of a controller method behind `@route`; the parameters of a public
model method (no leading `_`, no `@api.private`), which any user with access to the model can call
over RPC; and the request: `request.params`, `request.get_json_data()`,
`request.get_http_params()`, and the `args`, `form`, `values`, `files`, `cookies`, `headers`, `data`
and JSON of `request.httprequest`. A route parameter that a `model` or `models` converter of its URL
fills holds records, whose ids alone the user picks, and is no source.

A value stays tainted through assignments, attributes of `self` set in the same method, formatting,
concatenation, f-strings, string methods, items, attributes, collections, comprehensions and module
functions such as `json.loads()`. What a recordset method such as `browse()` or `search()` returns
is not tainted, but for OD712, which follows the records of `env[t]` or `env.get(t)` through locals,
`sudo()`, `browse()`, `exists()` and `filtered()`, and OD716, which follows the records `browse()`
reads by an id the user picks into what a route returns, until a check of them. OD717 follows the
`public` flag and the content of the values for `ir.attachment` through dict literals, `dict()`,
lists, locals and their items. Both branches of an `if` are followed, a branch that returns or
raises adds nothing after it, and a loop body is read twice.

A value passed to a method of `self`, of `env["x"]` or of a local bound to one is followed into
every definition of that method the module's dependencies hold, overrides included, up to four calls
deep; so is a value such a method returns, or returns from a method it calls. The finding is at the
sink when the module holds it, otherwise at the call that leaves the module. Records run as
superuser after `sudo()`, `sudo(SUPERUSER_ID)`, `with_user(SUPERUSER_ID)` or `with_user(1)`, and
so do those of an environment built by `env(su=True)`, `env(user=SUPERUSER_ID)` or
`Environment(cr, SUPERUSER_ID, ...)`, until `with_user()` for another user. `sudo(False)`, and a
`sudo(flag)` whose flag the code computes, end it too, except from saas~12.5 on the records of the
superuser, which Odoo runs in superuser mode whatever `su` is. A method called as superuser runs
so, and so do the methods it calls on `self`. For OD704, a mapping such a method is passed and
writes on `self`, or on the records of a call on `self`, is a superuser write; a mapping the method
reads from the request itself is not followed.

The flow ends at a sanitizer: `int()`, `float()`, `bool()`, `len()`, `SQL.identifier()`, `escape()`,
`html_escape()`, `html_sanitize()`, `plaintext2html()`, `urlencode()`, `url_encode()`,
`secure_filename()`, `content_disposition()`, `file_open()` and `Manifest.for_addon()`; at the
`name` of a file read from `request.httprequest.files` by a field the code names with `get()`,
`getlist()` or an item, which werkzeug sets to that field, in the method or a method it is passed
to; at `assert x in (...)` or `if x not in allowed: raise`, and inside a comprehension at its
`if x in allowed`; and at an intersection with values the code chose, such as `set(vals) & allowed`.
A test that `x` is in `env`, `registry`, `pool` or `_fields` proves only that the model or the
field exists, so it ends every flow but those of OD712, OD714 and OD715. Some steps make a value
harmless for one rule only: `os.path.basename()`, `io.BytesIO()`, `io.StringIO()` and a normalized
`os.path.join()` kept under a prefix the code picks for OD707, `shlex.quote()` and `shlex.join()`
for OD708, a start the code picks for OD705 and OD706, a name built with text of the code's for
OD713, and a condition whose field and operator the code picks for OD715, as their pages say. An id
stays the user's for OD716 through `int()` and `float()`, and ends at the access checks its page
lists.

A project adds its own sources, sinks and sanitizers in the
[`[taint]` table of its configuration](../configuration.md#taint-table).

| Code | Name | Reports |
| --- | --- | --- |
| [OD701](OD701.md) | `sql-injection-flow` | Data a user controls reaches the text of an SQL query. |
| [OD702](OD702.md) | `code-injection-flow` | Data a user controls reaches safe_eval(), eval() or exec(). |
| [OD703](OD703.md) | `markup-injection-flow` | Data a user controls is wrapped in Markup() without escaping. |
| [OD704](OD704.md) | `sudo-mass-assignment` | A mapping a user controls is written as superuser, so the user picks the fields. |
| [OD705](OD705.md) | `open-redirect-flow` | Data a user controls picks the site a redirect sends the browser to. |
| [OD706](OD706.md) | `server-request-forgery-flow` | Data a user controls picks the URL the server sends a request to. |
| [OD707](OD707.md) | `path-traversal-flow` | Data a user controls picks the path of a file the server opens. |
| [OD708](OD708.md) | `command-injection-flow` | Data a user controls reaches a command the server runs through the shell. |
| [OD709](OD709.md) | `unsafe-deserialization-flow` | Data a user controls is loaded by pickle, marshal or an unsafe YAML loader. |
| [OD710](OD710.md) | `xml-entity-flow` | Data a user controls is parsed as XML by a parser that resolves entities. |
| [OD711](OD711.md) | `header-injection-flow` | Data a user controls reaches the headers of a response. |
| [OD712](OD712.md) | `sudo-model-injection` | Data a user controls picks the model of records read or written as superuser. |
| [OD713](OD713.md) | `method-injection-flow` | Data a user controls names the method that getattr() looks up and the code calls. |
| [OD714](OD714.md) | `sudo-field-injection` | Data a user controls picks the field read as superuser. |
| [OD715](OD715.md) | `sudo-domain-injection` | Data a user controls picks the fields of a domain or a grouping searched as superuser. |
| [OD716](OD716.md) | `sudo-record-exposure` | A route returns records read as superuser by an id a user controls. |
| [OD717](OD717.md) | `sudo-public-attachment` | Data a user controls is stored as superuser in an attachment anyone may download. |
