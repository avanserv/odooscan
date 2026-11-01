from odoo import fields, models

for kind in ["first", "second"]:

    class OutsideBuilt(models.Model):
        _name = f"outside.built.{kind}"

        name = fields.Char()


class OutsideHolder(models.Model):
    _name = "outside.holder"

    large_id = fields.Many2one("bad.generated.large")
