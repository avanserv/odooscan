from odoo import fields, models

KINDS = ["small", "large", "huge"]

for kind in KINDS:

    class Generated(models.Model):
        _name = f"bad.generated.{kind}"

        name = fields.Char()
        size = fields.Integer()
        scale = fields.Integer()


class GeneratedUser(models.Model):
    _name = "bad.generated.user"

    small_id = fields.Many2one("bad.generated.small")
    large_id = fields.Many2one("bad.generated.large")

    def _small_records(self):
        return self.env["bad.generated.small"].search([])


class GeneratedHuge(models.Model):
    _inherit = "bad.generated.huge"

    label = fields.Char()


class GeneratedPaths(models.Model):
    _inherit = "bad.generated.user"
    _rec_names_search = ["large_id.scale", "large_id.name"]

    large_size = fields.Integer(related="large_id.size")
    large_name = fields.Char(related="large_id.name")
    user_label = fields.Char(related="no_generated_user")
