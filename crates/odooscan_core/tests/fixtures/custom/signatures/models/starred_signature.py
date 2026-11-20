from odoo import fields, models


class StarredSteps:
    def _stamp(self, order):
        return order


from os.path import *


class StarredSignature(models.Model, StarredSteps):
    _name = "starred.signature"
    _description = "Starred signature"

    name = fields.Char()

    def _prepare(self, order, line):
        return order, line
