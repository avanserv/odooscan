from odoo import fields, models


class SaasBand(models.AbstractModel):
    _name = "saas.band"
    _description = "SaaS band"

    model_id = fields.Many2one("ir.model", delegate=True, required=True)
