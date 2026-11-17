from odoo import models

from .util import Grid, Vector


class Handler(models.Model):
    _name = "x.plain.handler"
    _description = "Plain handler"

    handler = Vector()
    grid = Grid()

    def _not_fields(self):
        self.search([("handler", "=", False)])
        return self.search([("grid", "=", False)])
