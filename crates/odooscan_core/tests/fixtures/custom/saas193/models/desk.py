from odoo import fields, models


class SaasBeforeDesk(models.Model):
    _name = "saas.before.desk"
    _description = "SaaS before desk"

    name = fields.Char()
