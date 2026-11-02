from odoo import api, fields, models


class Left(models.Model):
    _name = "pair.left"

    right_id = fields.Many2one("pair.right")
    mirror = fields.Char(related="right_id.mirror")
    shown = fields.Char(compute="_compute_shown")

    @api.depends("mirror")
    def _compute_shown(self):
        self.shown = False


class Right(models.Model):
    _name = "pair.right"

    left_id = fields.Many2one("pair.left")
    mirror = fields.Char(related="left_id.mirror")
