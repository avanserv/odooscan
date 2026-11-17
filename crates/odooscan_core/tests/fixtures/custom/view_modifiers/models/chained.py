from odoo import fields, models

PARENTS = {"modifier.stamped": "stamped_id"}


class Noted:
    def action_note(self):
        return True


class ModifierStamped(models.Model):
    _name = "modifier.stamped"
    _description = "Holds the field a Python base gives"

    stamp = fields.Char()


class ModifierChained(ModifierStamped):
    _name = "modifier.chained"
    _inherit = ["modifier.stamped"]
    _description = "Python base that _inherit reads too"

    name = fields.Char()


class ModifierNoted(models.Model, Noted):
    _name = "modifier.noted"
    _description = "Python base that holds no field"

    name = fields.Char()


class ModifierUnreadInherits(models.Model):
    _name = "modifier.unread.inherits"
    _description = "An _inherits only a run reads"
    _inherits = PARENTS

    stamped_id = fields.Many2one("modifier.stamped", required=True, ondelete="cascade")
