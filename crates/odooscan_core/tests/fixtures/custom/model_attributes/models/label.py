from odoo import fields, models

REQUIRED = True


class GardenLabel(models.Model):
    _name = "garden.label"
    _description = "Garden label"
    _inherits = {"res.partner": "partner_id", "res.country": "country_id", "res.currency": "currency_id"}

    partner_id = fields.Many2one("res.partner", required=True)
    country_id = fields.Many2one("res.country", delegate=True, ondelete="restrict")
    currency_id = fields.Many2one("res.currency", required=True, ondelete="cascade")


class GardenSign(models.Model):
    _name = "garden.sign"
    _description = "Garden sign"
    _inherits = {"res.partner": "partner_id", "garden.plot": "plot_name", "garden.stage": "stage_id"}

    partner_id = fields.Many2one("res.partner", required=True)
    plot_name = fields.Char()


class GardenSignPartner(models.Model):
    _inherit = "garden.sign"

    partner_id = fields.Many2one(ondelete="cascade")


class GardenMarker(models.Model):
    _name = "garden.marker"
    _description = "Garden marker"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=REQUIRED, ondelete="cascade")


class GardenKit(models.Model):
    _name = "garden.kit"
    _description = "Garden kit"

    partner_id = fields.Many2one("res.partner", delegate=True)
    country_id = fields.Many2one("res.country", delegate=True, required=True)
    currency_id = fields.Many2one("res.currency", delegate=True, required=REQUIRED, ondelete="cascade")
    group_id = fields.Many2one("res.groups", delegate=True, required=True, ondelete="restrict")
    view_id = fields.Many2one("ir.ui.view", delegate=True, ondelete="set null")
    home_country_id = fields.Many2one("res.country", related="partner_id.country_id", delegate=True)


class GardenKitOwner(models.Model):
    _inherit = "garden.kit"

    partner_id = fields.Many2one(string="Owner", delegate=True)


class GardenTag(models.Model):
    _name = "garden.tag"
    _description = "Garden tag"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True)


class GardenTagPartner(models.Model):
    _inherit = "garden.tag"
    _inherits = {"res.partner": "partner_id"}
