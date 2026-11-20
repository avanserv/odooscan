from odoo import models

UPGRADED_MODEL = "unloaded.upgraded.model"


class UnloadedUpgradedModel(models.Model):
    _name = UPGRADED_MODEL


def migrate(cr, version):
    cr.execute("DELETE FROM unloaded_holder WHERE ghost_id IS NULL")
