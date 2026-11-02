from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv193_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
