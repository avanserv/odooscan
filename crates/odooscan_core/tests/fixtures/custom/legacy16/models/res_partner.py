from odoo import models


class ResPartner(models.Model):
    _inherit = "res.partner"
    _rec_names_search = ["name", "nickname"]
