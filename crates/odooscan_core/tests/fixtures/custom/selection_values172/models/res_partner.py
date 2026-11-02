from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv172_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
