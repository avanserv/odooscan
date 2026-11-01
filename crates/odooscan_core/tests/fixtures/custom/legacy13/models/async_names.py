from odoo import fields, models


class LegacyStampAsync(models.Model):
    _inherit = "legacy.stamp"

    async = fields.Boolean()

    def await(self):
        return self.async
