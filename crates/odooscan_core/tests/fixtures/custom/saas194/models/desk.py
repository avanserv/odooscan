from odoo import fields, models


class SaasAfterDesk(models.Model):
    _name = "saas.after.desk"
    _description = "SaaS after desk"

    name = fields.Char()
