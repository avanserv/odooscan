from odoo import fields, models


class Saas123Plain(models.Model):
    _name = "saas123.plain"
    _description = "SaaS 12.3 plain"

    name = fields.Char()


class Saas123Tracked(models.Model):
    _name = "saas123.tracked"
    _description = "SaaS 12.3 tracked"

    name = fields.Char()
    activity_ids = fields.Many2many("res.partner")
