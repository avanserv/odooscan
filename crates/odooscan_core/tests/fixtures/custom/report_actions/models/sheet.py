from odoo import api, models


class SheetReport(models.AbstractModel):
    _name = "report.report_actions.sheet_values"
    _description = "Sheet report values"

    @api.model
    def _get_report_values(self, docids, data=None):
        return {"docs": self.env["res.partner"].browse(docids)}
