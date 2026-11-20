from odoo import fields, models

OWNER_MODEL = "desk.team"
DELEGATED = True
BADGE_KEY = "badge_unique"
SERIAL_KEY = "serial_unique"
MIN_LEVEL = 1


class DeskTeam(models.Model):
    _name = "desk.team"
    _description = "Desk team"

    name = fields.Char()


class DeskCase(models.Model):
    _name = "desk.case"
    _description = "Desk case"
    _sql_constraints = [("code_unique", "unique(code)", "One case per code.")]

    name = fields.Char()
    code = fields.Char()
    ref = fields.Char()
    team_id = fields.Many2one("desk.team")
    stage_id = fields.Many2one("desk.team", index=True)
    queue_ref = fields.Char(index="btree_not_null")
    company_id = fields.Many2one("res.company")
    home_company_id = fields.Many2one("res.company", index=True)
    state = fields.Selection([("open", "Open"), ("closed", "Closed")])
    urgent = fields.Boolean()
    title = fields.Char(translate=True)
    total = fields.Integer(compute="_compute_total")
    tag_ids = fields.Many2many("desk.team")
    priority = fields.Integer()
    level = fields.Integer()
    score = fields.Float()
    sequence = fields.Integer()
    deadline = fields.Date()
    amount = fields.Float()
    parent_case_id = fields.Many2one("desk.case", auto_join=True)
    hours = fields.Integer()
    weight = fields.Float()
    opened_on = fields.Date()
    closed_at = fields.Datetime()
    currency_id = fields.Many2one("res.currency")
    fee = fields.Monetary()
    origin = fields.Reference([("desk.team", "Team")])
    res_model = fields.Char()
    res_id = fields.Many2oneReference(model_field="res_model")
    owner_id = fields.Many2one(OWNER_MODEL)
    team_name = fields.Char(related="team_id.name", store=True)
    parent_title = fields.Char(related="parent_case_id.title", store=True)
    owner_name = fields.Char(related="owner_id.name", store=True)
    owner_team_id = fields.Many2one("desk.team", related="owner_id.team_id", store=True)
    parent_team_name = fields.Char(related="parent_case_id.team_name", store=True)

    def _compute_total(self):
        for case in self:
            case.total = case.priority + case.level


class DeskCaseWizard(models.TransientModel):
    _name = "desk.case.wizard"
    _description = "Desk case wizard"

    team_id = fields.Many2one("desk.team")


class DeskCaseDigest(models.Model):
    _name = "desk.case.digest"
    _description = "Desk case digest"
    _auto = False

    team_id = fields.Many2one("desk.team")

    def _auto_init(self):
        return None


class DeskCaseLog(models.Model):
    _name = "desk.case.log"
    _description = "Desk case log"

    team_id = fields.Many2one("desk.team")

    def init(self):
        return None


class DeskCaseReport(models.Model):
    _name = "desk.case.report"
    _description = "Desk case report"
    _table_query = "SELECT id, team_id FROM desk_case"

    team_id = fields.Many2one("desk.team")


class Layout:
    note = fields.Char()


class DeskCaseShared(models.Model, Layout):
    _name = "desk.case.shared"
    _description = "Desk case shared"

    team_id = fields.Many2one("desk.team")


class DeskCaseDraft(models.Model):
    _name = "desk.case.draft"
    _description = "Desk case draft"

    team_id = fields.Many2one("desk.team")


class DeskSeat(models.Model):
    _name = "desk.seat"
    _description = "Desk seat"

    team_id = fields.Many2one("desk.team", delegate=DELEGATED, required=True, ondelete="cascade")


class DeskSlot(models.Model):
    _name = "desk.slot"
    _description = "Desk slot"
    _inherits = {"desk.team": "team_id"}

    team_id = fields.Many2one("desk.team", required=True, ondelete="cascade")


class DeskZone(models.Model):
    _name = "desk.zone"
    _description = "Desk zone"
    _parent_store = True

    parent_id = fields.Many2one("desk.zone")
    parent_path = fields.Char(index=True)
    team_id = fields.Many2one("desk.team")
    owner_zone_id = fields.Many2one("desk.zone")


class DeskFloor(models.Model):
    _name = "desk.floor"
    _description = "Desk floor"

    parent_id = fields.Many2one("desk.floor")


class DeskStampMixin(models.AbstractModel):
    _name = "desk.stamp.mixin"
    _description = "Desk stamp mixin"

    stamp_id = fields.Many2one("desk.team")

    def init(self):
        return None


class DeskStamp(models.Model):
    _name = "desk.stamp"
    _description = "Desk stamp"
    _inherit = ["desk.stamp.mixin"]

    team_id = fields.Many2one("desk.team")


class DeskCodeMixin(models.AbstractModel):
    _name = "desk.code.mixin"
    _description = "Desk code mixin"
    _sql_constraints = [("badge_unique", "unique(badge)", "One record per badge.")]

    badge = fields.Char()


class DeskBadge(models.Model):
    _name = "desk.badge"
    _description = "Desk badge"
    _inherit = ["desk.code.mixin"]

    holder_id = fields.Many2one("desk.team")


class DeskPass(models.Model):
    _name = "desk.pass"
    _description = "Desk pass"
    _inherit = ["desk.code.mixin"]
    _sql_constraints = [("badge_unique", "check(level > 0)", "Passes start at level one.")]

    level = fields.Integer()


class DeskCard(models.Model):
    _name = "desk.card"
    _description = "Desk card"
    _inherit = ["desk.code.mixin"]
    _sql_constraints = [("badge_unique", "check(true)", "Any card.")]


class DeskToken(models.Model):
    _name = "desk.token"
    _description = "Desk token"
    _inherit = ["desk.code.mixin"]
    # The scan cannot read a key named by a constant: Odoo replaces by key and drops the badge and
    # serial indexes, but the scan keeps both, so a rule on either stays silent.
    _sql_constraints = [
        (BADGE_KEY, "check(level > 0)", ""),
        (SERIAL_KEY, "unique(serial)", ""),
        ("serial_unique", "check(true)", ""),
    ]

    level = fields.Integer()
    serial = fields.Char()


class DeskVisa(models.Model):
    _name = "desk.visa"
    _description = "Desk visa"
    _inherit = ["desk.code.mixin"]
    _sql_constraints = [("badge_unique", "check(level > %s)" % MIN_LEVEL, "Above the minimum.")]

    level = fields.Integer()


class DeskPermit(models.Model):
    _name = "desk.permit"
    _description = "Desk permit"
    _inherit = ["desk.code.mixin"]
    _sql_constraints = [("badge_unique", "unique(badge, level)", "One per badge and level.")]

    level = fields.Integer()
