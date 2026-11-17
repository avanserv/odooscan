from odoo import fields, models


class ResPartner(models.Model):
    _name = "res.partner"

    name = fields.Char()
    type = fields.Selection([("contact", "Contact"), ("invoice", "Invoice")], default="contact")
