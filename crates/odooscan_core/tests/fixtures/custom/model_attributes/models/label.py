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


class GardenOwned(models.AbstractModel):
    _name = "garden.owned"
    _description = "Owned garden item"
    _inherits = {"res.partner": "partner_id", "res.country": "country_id"}

    partner_id = fields.Many2one("res.partner", required=True)
    country_id = fields.Many2one("res.country", required=True)


class GardenBench(models.Model):
    _name = "garden.bench"
    _inherit = ["garden.owned"]
    _description = "Garden bench"

    partner_id = fields.Many2one(delegate=True)
    country_id = fields.Many2one(ondelete="set null")


class GardenPost(models.Model):
    _name = "garden.post"
    _description = "Garden post"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True, required=True, ondelete="set null")
    home_country_id = fields.Many2one("res.country", delegate=False, required=True, ondelete="set null")
    parent_country_id = fields.Many2one("res.country", delegate=None, required=True, ondelete="set null")


class GardenHeld(models.AbstractModel):
    _name = "garden.held"
    _description = "Held garden item"
    _inherits = {"res.partner": "partner_id"}

    partner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")


class GardenLeased(models.AbstractModel):
    _name = "garden.leased"
    _description = "Leased garden item"
    _inherits = {"res.partner": "tenant_id"}

    tenant_id = fields.Many2one("res.partner", required=True, ondelete="cascade")


class GardenShed(models.AbstractModel):
    _name = "garden.shed"
    _inherit = ["garden.held"]
    _description = "Garden shed"
    _inherits = {"res.partner": "owner_id"}

    owner_id = fields.Many2one("res.partner", required=True, ondelete="cascade")
    partner_id = fields.Many2one(ondelete="set null")


class GardenHut(models.AbstractModel):
    _name = "garden.hut"
    _inherit = ["garden.held", "garden.leased"]
    _description = "Garden hut"

    partner_id = fields.Many2one(ondelete="set null")


CRATE_PARENTS = {"res.partner": "partner_id"}


class GardenCrate(models.AbstractModel):
    _name = "garden.crate"
    _description = "Garden crate"
    _inherits = CRATE_PARENTS

    partner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    country_id = fields.Many2one("res.country", delegate=True)
