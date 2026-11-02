from odoo import api, fields, models


class First(models.Model):
    _name = "ring.first"

    second_id = fields.Many2one("ring.second")
    target_id = fields.Many2one(related="second_id.target_id")
    shown = fields.Char(compute="_compute_shown")

    @api.depends("target_id.name")
    def _compute_shown(self):
        self.shown = False


class Second(models.Model):
    _name = "ring.second"

    third_id = fields.Many2one("ring.third")
    target_id = fields.Many2one(related="third_id.target_id")


class Third(models.Model):
    _name = "ring.third"

    first_id = fields.Many2one("ring.first")
    target_id = fields.Many2one(related="first_id.target_id")
