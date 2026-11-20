from odoo import models

from .util import Vector


class Handler(models.Model):
    _name = "x.tested.handler"
    _description = "Tested handler"

    handler = Vector()

    def _not_a_field(self):
        return self.search([("handler", "=", False)])
