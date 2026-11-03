from odoo import api, models


class ResPartnerUpgrade(models.Model):
    _inherit = "res.partner"

    def _x_note(self):
        self.message_post(body="Moved", subtype="mail.mt_note")
        self.message_post(body="Moved", subtype_xmlid="mail.mt_note")

    def _x_in_company(self, company):
        if self.env.context.get("force_company"):
            return self.with_context({"force_company": company.id})
        return self.with_context(force_company=company.id) | self.with_company(company)

    @api.onchange("parent_id")
    def _onchange_x_parent(self):
        return {"domain": {"child_ids": [("parent_id", "=", self.parent_id.id)]}}
