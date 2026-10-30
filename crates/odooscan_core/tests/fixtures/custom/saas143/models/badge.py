from odoo import fields, models


class SaasStrip(models.AbstractModel):
    _name = "saas.strip"
    _description = "SaaS strip"

    country_id = fields.Many2one("res.country", delegate=True, required=True)
    currency_id = fields.Many2one("res.currency", delegate=True)
    partner_id = fields.Many2one("res.partner", delegate=True, required=True, ondelete=None)
    company_id = fields.Many2one("res.company", delegate=True, required=True, ondelete=False)
    user_id = fields.Many2one("res.users", delegate=True, required=True, ondelete=0)
    group_id = fields.Many2one("res.groups", delegate=True, required=True, ondelete="")
    view_id = fields.Many2one("ir.ui.view", delegate=True, ondelete="")
    model_id = fields.Many2one("ir.model", delegate=True, required=True)


class SaasStripLabel(models.AbstractModel):
    _name = "saas.strip.label"
    _description = "SaaS strip label"

    model_id = fields.Many2one("ir.model", delegate=True, ondelete=None)
