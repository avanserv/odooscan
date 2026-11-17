from odoo import fields, models


class PlainHolder(models.Model):
    _name = "plain.holder"

    large_id = fields.Many2one("bad.generated.large")
    large_size = fields.Integer(related="large_id.no_plain_size")
    large_name = fields.Char(related="large_id.name")

    def _small_records(self):
        return self.env["bad.generated.small"].search([])
