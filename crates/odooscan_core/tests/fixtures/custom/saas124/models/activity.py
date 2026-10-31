from odoo import fields, models


class Saas124Plain(models.Model):
    _name = "saas124.plain"
    _description = "SaaS 12.4 plain"

    name = fields.Char()


class Saas124Tracked(models.Model):
    _name = "saas124.tracked"
    _description = "SaaS 12.4 tracked"

    name = fields.Char()
    activity_ids = fields.Many2many("res.partner")
