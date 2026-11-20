from odoo import models

STAGED_MODEL = "outside.staged.model"


class OutsideStagedModel(models.Model):
    _name = STAGED_MODEL


def migrate(cr, version):
    cr.execute("UPDATE outside_holder SET large_id = NULL")
