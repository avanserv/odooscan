from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("gadget_tier", "Gadget tier")], ondelete={"gadget_tier": "cascade"})
