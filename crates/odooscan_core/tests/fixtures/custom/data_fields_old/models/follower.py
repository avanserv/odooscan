from odoo import fields, models


class OldFollower(models.Model):
    _name = "old.follower"
    _description = "Follower"

    name = fields.Char()
    reference = fields.Many2oneReference(model_field="x_res_model")
