from odoo import models

from .side_mixins import SideStamp


class DomainChained(models.Model, SideStamp):
    _inherit = "domain.chained"
