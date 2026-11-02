from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv13_state = fields.Selection([("draft", "Draft"), ("done", "Done")])
