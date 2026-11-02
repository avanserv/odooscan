from odoo import fields, models


class Hazy(models.AbstractModel):
    _name = "inherited.outside.hazy"

    tag = fields.Char()

    def report(self):
        print "x"
