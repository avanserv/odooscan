from mixin_kit import ImportedMixin

from odoo import models


class Base(models.AbstractModel, ImportedMixin):
    _inherit = "base"
