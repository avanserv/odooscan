from odoo import fields, models


# The stored field of m2m_rows keeps its rows in the table SHELF_TAGS holds, which may be this one.
class ShelfNote(models.Model):
    _name = "x.shelf.note"
    _description = "Shelf note link"
    _table = "x_shelf_note_rel"
    _auto = False

    x_shelf_id = fields.Many2one("x.shelf", readonly=True)
    res_country_id = fields.Many2one("res.country", readonly=True)
