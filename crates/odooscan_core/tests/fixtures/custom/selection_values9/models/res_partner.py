from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv9_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
    sv9_state = fields.Selection([("draft", "Draft"), ("done", "Done")])
