from odoo import fields, models


class OldFollower(models.Model):
    _name = "old.follower"
    _description = "Follower"

    name = fields.Char()
    reference = fields.Many2oneReference(model_field="x_res_model")


class OldFollowerSetup(models.Model):
    _inherit = "old.follower"

    # Odoo runs _init_column and init_storage only on a table that holds rows, as on the
    # upgrade that adds this field.
    seeded = fields.Char(init_storage="_init_seeded")

    def _init_column(self, column_name):
        self.env.cr.execute("SELECT f.x_res_model FROM old_follower f")

    def _init_seeded(self, records):
        self.env.cr.execute("SELECT f.x_res_model FROM old_follower f")
