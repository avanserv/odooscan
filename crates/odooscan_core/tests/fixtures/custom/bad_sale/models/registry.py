from odoo import fields, models


class RegistryOrder(models.Model):
    _name = "bad.registry"
    _order = "stat desc, id"
    _rec_name = "titel"

    partner_id = fields.Many2one("res.partner")
    country_name = fields.Char(related="partner_id.country_id")
    partner_label = fields.Char(related="partner_id.name")
    note_ids = fields.One2many("bad.registry.note", "label")
    record_note_ids = fields.One2many("bad.registry.note", "res_id")
    order_note_ids = fields.One2many("bad.registry.note", "registry_id")


class RegistryNote(models.Model):
    _name = "bad.registry.note"
    _order = "COALESCE(label, '')"

    label = fields.Char()
    res_id = fields.Integer()
    registry_id = fields.Many2one("bad.registry")


class RegistryPartner(models.Model):
    _name = "res.partner"
    _description = "Contact"


class RegistryCountry(models.Model):
    _name = "res.country"
    _inherit = _name

    fee = fields.Monetary()
