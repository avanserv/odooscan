from odoo import models


class ResCompany(models.Model):
    _inherit = "res.company"

    def _check_selection_field_value(self, field, value):
        return super()._check_selection_field_value(field, value)

    def _check_record_rules_result_count(self, result_ids, operation):
        return super()._check_record_rules_result_count(result_ids, operation)

    def _table_exist(self):
        return super()._table_exist()
