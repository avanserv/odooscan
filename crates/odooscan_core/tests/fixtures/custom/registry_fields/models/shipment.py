from odoo import fields, models

CARRIER = "res.partner"
COUNTRY = "res.country"


class RegistryShipment(models.Model):
    _name = "registry.shipment"
    _description = "Shipment"

    reference = fields.Char(required=True)
    handler_id = fields.Many2one("res.partner")
    carrier_ids = fields.Many2many("res.partner")
    sender_ids = fields.Many2many(CARRIER)
    origin_ids = fields.Many2many(COUNTRY)
    zone_ids = fields.Many2many("res.country")
    linked_ids = fields.Many2many("registry.shipment")
    merged_ids = fields.Many2many("registry.shipment")


class RegistryShipmentCarrier(models.Model):
    _inherit = "registry.shipment"

    reference = fields.Many2one("res.partner", ondelete="set null")
    handler_id = fields.Many2one(required=True, ondelete="set null")
    carrier_ids = fields.Many2one("res.partner")
    driver_ids = fields.Many2many("res.partner")


class RegistryBadge(models.Model):
    _name = "registry.badge"
    _description = "Shipment badge"
    _inherits = {"registry.shipment": "shipment_id"}

    shipment_id = fields.Many2one("registry.shipment", required=True, ondelete="cascade")
    region_ids = fields.Many2many("res.country")


class RegistryVoucherBase(models.AbstractModel):
    _register = False

    currency_id = fields.Many2one("res.currency")


class RegistryVoucher(RegistryVoucherBase):
    _name = "registry.voucher"
    _description = "Voucher"

    amount = fields.Monetary()


class RegistryTracked(models.AbstractModel):
    _name = "registry.tracked"
    _description = "Tracked by a partner"

    partner_id = fields.Many2one("res.partner", ondelete="cascade")


class RegistryParcel(models.Model):
    _name = "registry.parcel"
    _description = "Parcel"

    partner_id = fields.Many2one("res.partner", ondelete="set null")


class RegistryParcelTracked(models.Model):
    _name = "registry.parcel"
    _inherit = ["registry.tracked", "registry.parcel"]

    partner_id = fields.Many2one(required=True)
