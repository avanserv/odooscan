from odoo import api, fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"
    _rec_names_search = ["name", "nickname"]


class ResPartnerCreate(models.Model):
    _inherit = "res.partner"

    @api.model
    def create(self, vals):
        vals.setdefault("ref", vals.get("name"))
        return super().create(vals)


class ResPartnerStamp(models.Model):
    _inherit = "res.partner"

    create_date = fields.Datetime()
    opened_on = fields.Datetime(string="Created on")
    touched_on = fields.Datetime(string="Last Modified on")


class LegacyTurnstile(models.Model):
    _name = "legacy.turnstile"
    _description = "Legacy turnstile"

    __last_update = fields.Datetime(string="Turned")
    turned_on = fields.Datetime(string="Last Modified on")
