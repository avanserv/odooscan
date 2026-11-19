from odoo import models

STAGED_MODEL = "unloaded.staged.model"


class UnloadedStagedModel(models.Model):
    _name = STAGED_MODEL


def migrate(cr, version):
    cr.execute("UPDATE unloaded_holder SET ghost_id = NULL")
