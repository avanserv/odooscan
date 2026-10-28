from odoo import fields, models

KINDS = ["small", "large"]

for kind in KINDS:

    class Generated(models.Model):
        _name = f"bad.generated.{kind}"

        name = fields.Char()


class GeneratedUser(models.Model):
    _name = "bad.generated.user"

    small_id = fields.Many2one("bad.generated.small")
