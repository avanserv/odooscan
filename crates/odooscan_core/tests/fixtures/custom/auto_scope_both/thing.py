from odoo import fields, models


class AutoScopeThing(models.Model):
    _inherit = "auto.scope.thing"

    both_stamp = fields.Char()
    both_next_id = fields.Many2one("auto.scope.thing")


class AutoMadeOne(models.AbstractModel):
    _name = "auto.made.one"
    _description = "Model a module of the depends builds under the same name"
