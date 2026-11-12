from odoo import fields, models


class ExposureCounter(models.Model):
    _name = "exposure192.counter"
    _description = "Counter portal users read"

    name = fields.Char()


class ExposureTally(models.Model):
    _name = "exposure192.tally"
    _description = "Tally every user writes"

    name = fields.Char()
