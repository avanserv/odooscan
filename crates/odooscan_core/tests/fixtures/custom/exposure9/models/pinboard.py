from openerp import fields, models


class Exposure9Pinboard(models.Model):
    _name = "exposure9.pinboard"
    _description = "Pinboard a portal user reads"

    name = fields.Char()
