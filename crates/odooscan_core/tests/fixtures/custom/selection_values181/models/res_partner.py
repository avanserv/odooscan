from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sv181_mode = fields.Selection((("draft", "Draft"), ("done", "Done")))
    sv181_gate = fields.Selection((("open", "Open"),), required=True, default="open")


class ResPartnerGate(models.Model):
    _inherit = "res.partner"

    sv181_gate = fields.Selection(selection_add=[("shut", "Shut")])
