from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv17_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
    sv17_gate = fields.Selection((("open", "Open"),), required=True, default="open")


class ResPartnerGate(models.Model):
    _inherit = "res.partner"

    sv17_gate = fields.Selection(selection_add=[("shut", "Shut")])
