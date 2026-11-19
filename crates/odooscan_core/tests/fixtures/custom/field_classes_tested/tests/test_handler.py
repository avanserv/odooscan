from odoo import models
from odoo.addons.field_classes.orm.vector import Vector


class Probe(models.Model):
    _name = "x.tested.probe"
    _description = "Tested probe"

    probe = Vector()

    def _a_field(self):
        return self.search([("probe", "=", False)])
