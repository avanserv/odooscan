from odoo import fields, models


class PlainHolder(models.Model):
    _name = "plain.holder"

    large_id = fields.Many2one("bad.generated.large")
