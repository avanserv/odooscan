from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    sa171_auto_stage = fields.Selection(selection_add=[("held", "Held")], ondelete={"held": "cascade"})
