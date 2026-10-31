from odoo import models

for kind in ["small", "large"]:

    class RvGenerated(models.Model):
        _name = f"rv.generated.{kind}"
        _description = "Generated"
