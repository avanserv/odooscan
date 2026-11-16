from openerp import fields, models


class Exposure9Saas10Pinboard(models.Model):
    _name = "exposure9_saas10.pinboard"
    _description = "Pinboard a portal user reads"

    name = fields.Char()
