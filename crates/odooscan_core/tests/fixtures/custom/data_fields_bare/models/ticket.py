from odoo import fields, models


# Without a series the scan cannot tell whether _auto_init fills parent_path, so _parent_name
# resolves against data/fields.xml.
class DataTicketCopy(models.Model):
    _inherit = "data.ticket.copy"
    _parent_store = True
    _parent_name = "x_bare_parent_id"

    parent_path = fields.Char(index=True)
