from odoo import fields, models


class LegacyDigest(models.Model):
    _name = "legacy.digest"
    _description = "Legacy digest"
    _auto = False

    entry_count = fields.Integer()
