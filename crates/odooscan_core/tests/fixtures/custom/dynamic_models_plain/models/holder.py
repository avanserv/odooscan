from odoo import fields, models


class PlainHolder(models.Model):
    _name = "plain.holder"

    large_id = fields.Many2one("bad.generated.large")

    def _small_records(self):
        return self.env["bad.generated.small"].search([])
