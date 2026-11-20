from odoo import models

UPGRADED_MODEL = "outside.upgraded.model"


class OutsideUpgradedModel(models.Model):
    _name = UPGRADED_MODEL


def migrate(cr, version):
    cr.execute("DELETE FROM outside_holder WHERE large_id IS NULL")
