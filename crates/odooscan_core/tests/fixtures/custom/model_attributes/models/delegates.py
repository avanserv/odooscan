from odoo import fields, models

GATE_ONDELETE = "cascade"
GATE_OPTIONS = {"ondelete": "cascade"}


class GardenGate(models.AbstractModel):
    _name = "garden.gate"
    _description = "Garden gate"

    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete=None)
    partner_id = fields.Many2one("res.partner", delegate=True, ondelete=False)
    currency_id = fields.Many2one("res.currency", delegate=True, required=True, ondelete="")
    group_id = fields.Many2one("res.groups", delegate=True, required=True, ondelete=GATE_ONDELETE)
    view_id = fields.Many2one("ir.ui.view", delegate=True, required=True, **GATE_OPTIONS)
    home_country_id = fields.Many2one("res.country", delegate=False, required=True)
    parent_country_id = fields.Many2one("res.country", delegate=None, required=True)


class GardenLock(models.AbstractModel):
    _name = "garden.lock"
    _description = "Garden lock"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete=None)


class GardenHinge(models.AbstractModel):
    _name = "garden.hinge"
    _description = "Garden hinge"

    country_id = fields.Many2one("res.country", delegate=True, required=True)


class GardenHingeCountry(models.AbstractModel):
    _inherit = "garden.hinge"

    country_id = fields.Many2one(ondelete="restrict")
