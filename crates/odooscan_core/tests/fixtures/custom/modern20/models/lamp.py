from odoo import fields, models


class ModernLamp(models.Model):
    _inherit = ["modern.lamp"]

    x_kept = fields.Char()

    def x_kept_values(self):
        self.env.cr.execute("SELECT l.x_kept FROM modern_lamp l")
        return self.env.cr.fetchall()
