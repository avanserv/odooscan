from odoo import _, fields, models

KW = {"readonly": True}
LABELS = {"a": "Named elsewhere"}
STATES = [("a", "Ay")]
OTHER_MODEL = "rv." + "other"
INVERSE = "thing" + "_id"
TABLE = "rv_" + "computed"


class PlainMixin:
    pass


class RvOther(models.Model):
    _name = "rv.other"
    _description = "Other"

    name = fields.Char()
    code = fields.Selection([("c", "Cee"), ("d", "Dee")])


class RvShared(models.Model):
    _name = "rv.shared"
    _description = "Shared"
    _table = "rv_other"

    name = fields.Char()


class RvComputed(models.Model):
    _name = "rv.computed"
    _description = "Computed table"
    _table = TABLE

    name = fields.Char()


class RvBlended(PlainMixin, models.Model):
    _name = "rv.blended"
    _description = "Blended"

    other_id = fields.Many2one("rv.other")
    state = fields.Selection([("a", "Ay")])


class RvAction(models.Model):
    _name = "rv.action"
    _description = "Action"

    name = fields.Char()


class RvActionWindow(models.Model):
    _name = "rv.action.window"
    _inherit = "rv.action"
    _description = "Window action"

    extra = fields.Char()


class RvParent(models.Model):
    _name = "rv.parent"
    _description = "Parent"

    name = fields.Char()
    kind = fields.Selection([("x", "Ex"), ("y", "Why")])
    twin_id = fields.Many2one("rv.other")
    shade_id = fields.Many2one("rv.other")


class RvChild(models.Model):
    _name = "rv.child"
    _description = "Child"
    _inherits = {"rv.parent": "parent_id"}

    parent_id = fields.Many2one("rv.parent", required=True, ondelete="cascade")
    shade_id = fields.Many2one("rv.parent")


class RvLine(models.Model):
    _name = "rv.line"
    _description = "Line"

    thing_id = fields.Many2one("rv.thing")
    other_id = fields.Many2one("rv.other")
    state = fields.Selection([("a", "Ay")])


class RvThing(models.Model):
    _name = "rv.thing"
    _description = "Thing"

    name = fields.Char()
    parent_id = fields.Many2one("rv.parent")
    other_id = fields.Many2one("rv.other")
    other_ids = fields.Many2many("rv.other", string="Others")
    line_ids = fields.One2many("rv.line", "thing_id")
    loose_line_ids = fields.One2many("rv.line", INVERSE)
    action_id = fields.Many2one("rv.action")
    target_named = fields.Reference([("rv.other", "Other")], comodel_name="rv.other")
    res_named_id = fields.Many2oneReference(model_field="res_model", comodel_name="rv.other")
    res_model = fields.Char()
    count_named = fields.Integer(comodel_name="rv.other")
    related_other_id = fields.Many2one(related="parent_id.twin_id", store=True)
    computed_other_id = fields.Many2one("rv.other", compute="_compute_other_id")
    named_id = fields.Many2one(comodel_name=OTHER_MODEL)
    spread_id = fields.Many2one("rv.other", **KW)
    spread_bare_id = fields.Many2one(store=True, **KW)
    side_id = fields.Many2one("rv.side.only")
    retyped_id = fields.Many2one("rv.other")
    fallen_id = fields.Many2one("rv.other")
    state = fields.Selection([("a", "Ay"), ("b", _("Bee"))])
    rated = fields.Selection([("a", LABELS["a"])])
    by_method = fields.Selection("_list_kinds")
    by_lambda = fields.Selection(lambda self: self._list_kinds())
    by_variable = fields.Selection(STATES)
    by_int = fields.Selection([(1, "One")])
    by_tuple = fields.Selection((("a", "Ay"), ("b", "Bee")))
    unchecked = fields.Selection([("a", "Ay")], validate=False)
    related_kind = fields.Selection([("a", "Ay")], related="parent_id.kind", store=True)
    computed_kind = fields.Selection([("a", "Ay")], compute="_compute_kind")
    lone = fields.Selection(selection_add=[("a", "Ay")])
    added = fields.Selection([("a", "Ay")])
    spread_kind = fields.Selection([("a", "Ay")])

    def _compute_other_id(self):
        self.computed_other_id = False

    def _compute_kind(self):
        self.computed_kind = "a"

    def _list_kinds(self):
        return STATES
