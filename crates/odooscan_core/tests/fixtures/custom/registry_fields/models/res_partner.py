from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("site", "Site")])


class ResPartnerDepot(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("depot", "Depot")], ondelete={"depot": "set null"})


class ResPartnerPickup(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("pickup", "Pickup")], ondelete={"pickup": "cascade"})


class ResPartnerOrder(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("invoice",), ("contact",)])


class ResPartnerReinvoice(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("invoice",)], ondelete={"invoice": "set null"})
