from odoo import fields, models


class EncodedMemo(models.Model):
    _name = "encoded.memo"
    _description = "Memo"

    title = fields.Char(string="Résumé")
