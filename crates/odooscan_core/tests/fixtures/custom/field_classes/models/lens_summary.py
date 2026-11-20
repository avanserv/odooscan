from odoo import fields, models, tools
from odoo.addons.field_classes.orm.vector import Pair


class LensSummary(models.Model):
    _name = "x.lens.summary"
    _description = "Lens summary"
    _auto = False

    name = fields.Char(readonly=True)
    note = fields.Char(readonly=True)
    # Pair is a Char or a Many2many, which has no column, so whether the view must select one for
    # pair is unknown.
    pair = Pair(readonly=True)

    def init(self):
        tools.drop_view_if_exists(self.env.cr, self._table)
        self.env.cr.execute(
            """
            CREATE OR REPLACE VIEW x_lens_summary AS (
                SELECT l.id, l.code AS name
                  FROM x_lens l
            )
            """
        )
