from odoo import fields, models


class SaasMemo(models.AbstractModel):
    _name = "saas.memo"
    _description = "SaaS memo"

    stage = fields.Char(tracking=True)
