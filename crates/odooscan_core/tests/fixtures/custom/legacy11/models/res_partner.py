from odoo import fields, models


# Before 12.0 _auto_init fills no parent_path column, so _parent_name resolves against
# data/fields.xml.
class ResPartner(models.Model):
    _inherit = "res.partner"
    _parent_store = True
    _parent_name = "x_legacy_parent_id"

    parent_path = fields.Char(index=True)
