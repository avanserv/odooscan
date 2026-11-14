from odoo import models


class PartnerLate(models.Model):
    _inherit = "res.partner"


def setup(env):
    return env
