from odoo import fields, models


class CsvRows(models.Model):
    _name = "csv.rows"
    _description = "CSV rows"

    name = fields.Char()
