from odoo import fields, models


class DrawerSummary(models.Model):
    _name = "drawer.summary"
    _description = "Drawer summary"
    _auto = False

    drawer_count = fields.Integer()
