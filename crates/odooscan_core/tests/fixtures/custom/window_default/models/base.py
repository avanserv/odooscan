from odoo import models


class Base(models.AbstractModel):
    _inherit = "base"

    def _get_default_gantt_view(self):
        return None
