from odoo import api, fields, models

COND = 1 > 0
KW = {"readonly": True}
PARENTS = {"rr.parent": "parent_id"}
DEFAULTS = {"code": "c"}


class PlainMixin:
    pass


class RrTag(models.Model):
    _name = "rr.tag"
    _description = "Tag"

    name = fields.Char()


class RrNote(models.Model):
    _name = "rr.note"
    _description = "Note"

    name = fields.Char()
    code = fields.Char()
    tag_ids = fields.Many2many("rr.tag")
    entry_ids = fields.One2many("rr.entry", "note_id")


class RrEntry(models.Model):
    _name = "rr.entry"
    _description = "Entry"

    note_id = fields.Many2one("rr.note")


class RrLine(models.Model):
    _name = "rr.line"
    _description = "Line"

    thing_id = fields.Many2one("rr.thing", required=True, ondelete="cascade")
    label = fields.Char(required=True)
    tag_id = fields.Many2one("rr.tag")


class RrMixin(models.AbstractModel):
    _name = "rr.mixin"
    _description = "Mixin"

    mixed = fields.Char(required=True)
    mixed_default = fields.Char(required=True, default="m")


class RrThing(models.Model):
    _name = "rr.thing"
    _description = "Thing"

    def _default_by_method(self):
        return "x"

    name = fields.Char(required=True)
    note = fields.Char()
    defaulted = fields.Char(required=True, default="d")
    zero = fields.Integer(required=True, default=0)
    by_lambda = fields.Char(required=True, default=lambda self: "x")
    by_method = fields.Char(required=True, default=_default_by_method)
    computed = fields.Char(required=True, compute="_compute_computed", store=True)
    related_name = fields.Char(required=True, related="name", store=True)
    unstored = fields.Char(required=True, store=False)
    tag_id = fields.Many2one("rr.tag", string="Main tag")
    line_ids = fields.One2many("rr.line", "thing_id", required=True)
    tag_ids = fields.Many2many("rr.tag", required=True)
    attached = fields.Binary(required=True)
    picture = fields.Image(required=True)
    by_company = fields.Char(required=True, company_dependent=True)
    sequence = fields.Integer(required=True)
    xmlid = fields.Char(required=True)
    spread = fields.Char(required=True, **KW)
    optional = fields.Char(required=False)
    conditional = fields.Char(required=COND)

    def _compute_computed(self):
        self.computed = "x"


class RrMixed(models.Model):
    _name = "rr.mixed"
    _inherit = ["rr.mixin"]
    _description = "Mixed"

    name = fields.Char()


class RrScoped(models.Model):
    _name = "rr.scoped"
    _description = "Extended by a module that depends on this one"

    name = fields.Char(required=True)


class RrDefaulted(models.Model):
    _name = "rr.defaulted"
    _description = "Given a default by ir.default"

    name = fields.Char(required=True)


class RrNamed(models.Model):
    _name = "rr.named"
    _description = "A method named like a default"

    code = fields.Char(required=True)

    def _default_code(self):
        return "x"


class RrLegacy(models.Model):
    _name = "rr.legacy"
    _description = "Legacy defaults by name"
    _defaults = {"code": "c"}

    code = fields.Char(required=True)


class RrLegacyHidden(models.Model):
    _name = "rr.legacy.hidden"
    _description = "Legacy defaults by expression"
    _defaults = DEFAULTS

    code = fields.Char(required=True)


class RrHidden(PlainMixin, models.Model):
    _name = "rr.hidden"
    _description = "A parent the scan cannot see"

    name = fields.Char(required=True)


class RrView(models.Model):
    _name = "rr.view"
    _description = "On a view"
    _auto = False

    name = fields.Char(required=True)

    def init(self):
        self.env.cr.execute("CREATE VIEW rr_view AS (SELECT 1 AS id, 'a' AS name)")


class RrHookDefaultGet(models.Model):
    _name = "rr.hook.default_get"
    _description = "Hook"

    name = fields.Char(required=True)

    def default_get(self, fields_list):
        return super().default_get(fields_list)


class RrHookCreate(models.Model):
    _name = "rr.hook.create"
    _description = "Hook"

    name = fields.Char(required=True)

    @api.model_create_multi
    def create(self, vals_list):
        return super().create(vals_list)


class RrHookLoad(models.Model):
    _name = "rr.hook.load"
    _description = "Hook"

    name = fields.Char(required=True)

    def load(self, fields, data):
        return super().load(fields, data)


class RrHookMissing(models.Model):
    _name = "rr.hook.missing"
    _description = "Hook"

    name = fields.Char(required=True)

    def _add_missing_default_values(self, values):
        return super()._add_missing_default_values(values)


class RrHookPrepare(models.Model):
    _name = "rr.hook.prepare"
    _description = "Hook"

    name = fields.Char(required=True)

    def _prepare_create_values(self, vals_list):
        return super()._prepare_create_values(vals_list)


class RrHookRecords(models.Model):
    _name = "rr.hook.records"
    _description = "Hook"

    name = fields.Char(required=True)

    def _load_records(self, data_list, update=False, ignore_duplicates=False):
        return super()._load_records(data_list, update, ignore_duplicates)


class RrHookRecordsCreate(models.Model):
    _name = "rr.hook.records_create"
    _description = "Hook"

    name = fields.Char(required=True)

    def _load_records_create(self, vals_list):
        return super()._load_records_create(vals_list)


class RrFieldHookMixin(models.AbstractModel):
    _name = "rr.field.hook.mixin"
    _description = "Holds the field and fills it"

    hooked = fields.Char(required=True)

    def default_get(self, fields_list):
        return super().default_get(fields_list)


class RrFieldHooked(models.Model):
    _name = "rr.field.hooked"
    _inherit = ["rr.field.hook.mixin"]
    _description = "Filled by a parent that has the field"


class RrBlindMixin(models.AbstractModel):
    _name = "rr.blind.mixin"
    _description = "Has a hook, and not the field"

    def default_get(self, fields_list):
        return super().default_get(fields_list)


class RrBlind(models.Model):
    _name = "rr.blind"
    _inherit = ["rr.blind.mixin"]
    _description = "A hook of a parent without the field fills nothing"

    name = fields.Char(required=True)


class RrParent(models.Model):
    _name = "rr.parent"
    _description = "Parent"

    label = fields.Char(required=True)


class RrChild(models.Model):
    _name = "rr.child"
    _description = "Child"
    _inherits = {"rr.parent": "parent_id"}

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade")


class RrGrandChild(models.Model):
    _name = "rr.grandchild"
    _description = "Grandchild"
    _inherits = {"rr.child": "child_id"}

    child_id = fields.Many2one("rr.child", required=True, ondelete="cascade")


class RrFlagged(models.Model):
    _name = "rr.flagged"
    _description = "Delegates by a flag on the field"

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade", delegate=True)


class RrSpared(models.Model):
    _name = "rr.spared"
    _description = "A delegate field with a default creates no parent"
    _inherits = {"rr.parent": "parent_id"}

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade", default=lambda self: 1)


class RrHookedChild(models.Model):
    _name = "rr.hooked.child"
    _description = "A hook of the child may fill the fields of its parent"
    _inherits = {"rr.parent": "parent_id"}

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade")

    def default_get(self, fields_list):
        return super().default_get(fields_list)


class RrUnreadChild(models.Model):
    _name = "rr.unread.child"
    _description = "An _inherits only a run reads"
    _inherits = PARENTS

    name = fields.Char(required=True)


class RrLoopA(models.Model):
    _name = "rr.loop.a"
    _description = "Delegates in a cycle"
    _inherits = {"rr.loop.b": "b_id"}

    b_id = fields.Many2one("rr.loop.b", required=True, ondelete="cascade")
    name_a = fields.Char(required=True)


class RrLoopB(models.Model):
    _name = "rr.loop.b"
    _description = "Delegates in a cycle"
    _inherits = {"rr.loop.a": "a_id"}

    a_id = fields.Many2one("rr.loop.a", required=True, ondelete="cascade")
    name_b = fields.Char(required=True)


class RrChildDefault(models.Model):
    _name = "rr.child.default"
    _description = "Its default of a parent field goes to the parent it creates"
    _inherits = {"rr.parent": "parent_id"}

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade")
    label = fields.Char(related="parent_id.label", inherited=True, readonly=False, default="x")


class RrGrandChildDefault(models.Model):
    _name = "rr.grandchild.default"
    _description = "Takes the default of a parent field from its child"
    _inherits = {"rr.child.default": "child_id"}

    child_id = fields.Many2one("rr.child.default", required=True, ondelete="cascade")


class RrChildShadow(models.Model):
    _name = "rr.child.shadow"
    _description = "Without inherited its default stays out of the parent it creates"
    _inherits = {"rr.parent": "parent_id"}

    parent_id = fields.Many2one("rr.parent", required=True, ondelete="cascade")
    label = fields.Char(related="parent_id.label", readonly=False, default="x")
