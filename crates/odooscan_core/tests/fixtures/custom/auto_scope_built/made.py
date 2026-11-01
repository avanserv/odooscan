from odoo import models

KINDS = ["one", "two"]

for kind in KINDS:

    class AutoMade(models.AbstractModel):
        _name = f"auto.made.{kind}"
        _description = "Model built at runtime"

        def _extras(self):
            return self.env["auto.made.one"].search([("extra", "=", 1)])
