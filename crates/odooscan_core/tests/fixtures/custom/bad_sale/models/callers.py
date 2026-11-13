from odoo import models


class SaleOrderCallers(models.Model):
    _inherit = "bad.order"

    def _apply_values(self, vals):
        self.write(vals)

    def _note_html(self, note):
        return self._note_paragraph(note)

    def _note_paragraph(self, note):
        return "<p>%s</p>" % note

    def apply_values(self, vals):
        self.write(vals)
