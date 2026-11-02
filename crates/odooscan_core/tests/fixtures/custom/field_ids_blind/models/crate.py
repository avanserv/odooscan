from odoo import fields, models


class CrateBlind(models.Model):
    _inherit = "ghost.crate"

    weight = fields.Integer()
    print "unread"
