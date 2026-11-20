from odoo import fields, models


# Neither dependency field named group_ids gives a table the scan reads: m2m_rows_user names it
# through BIN_TAGS, m2m_rows_related through its keywords. The scan counts such a field of a
# dependency only when Odoo surely stores it, and neither is: a related unsets store in one, and the
# keywords may hide one in the other. So this model is reported.
class BinNote(models.Model):
    _name = "x.bin.note"
    _description = "Bin note link"
    _table = "x_bin_note_rel"
    _auto = False

    x_bin_id = fields.Many2one("x.bin", readonly=True)
    res_groups_id = fields.Many2one("res.groups", readonly=True)
