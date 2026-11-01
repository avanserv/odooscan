from odoo import fields, models

MODEL_NAME = "opaque.built"


class OpaqueBuilt(models.Model):
    _name = MODEL_NAME

    name = fields.Char()


class OpaqueHolder(models.Model):
    _name = "opaque.holder"

    large_id = fields.Many2one("bad.generated.large")

    def _built_records(self):
        return self.env["opaque.built"].search([])
