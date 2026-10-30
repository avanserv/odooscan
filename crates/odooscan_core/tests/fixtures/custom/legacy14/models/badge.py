from odoo import fields, models


class LegacyBadge(models.AbstractModel):
    _name = "legacy.badge"
    _description = "Legacy badge"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
    id = fields.Integer(readonly=True)
    ref = fields.Char(string="ID")


class LegacyPlate(models.AbstractModel):
    _name = "legacy.plate"
    _description = "Legacy plate"
    _inherits = {"res.partner": "partner_id", "res.currency": "currency_id"}

    partner_id = fields.Many2one("res.partner", required=True)
    currency_id = fields.Many2one("res.currency")


class LegacyPass(models.AbstractModel):
    _name = "legacy.pass"
    _description = "Legacy pass"
    _inherits = {"res.partner": "partner_id", "res.currency": "currency_id", "ir.model": "model_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete=None)
    currency_id = fields.Many2one("res.currency", required=True, ondelete="")
    model_id = fields.Many2one("ir.model", required=True)


class LegacyPassWizard(models.TransientModel):  # odooscan: ignore[OD401]
    _name = "legacy.pass.wizard"
    _description = "Legacy pass wizard"

    model_id = fields.Many2one("ir.model", delegate=True, required=True)
