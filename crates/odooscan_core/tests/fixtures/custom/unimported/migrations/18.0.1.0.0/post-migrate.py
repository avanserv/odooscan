from odoo import models


class PartnerMigrated(models.AbstractModel):
    _inherit = "res.partner"


def migrate(cr, version):
    pass
