from odoo import _, _lt, fields, models

LATE = _("This order is late")
LABEL = _lt("Order")
TOTAL = _lt("%s of %s", "total", "orders", _module="bad_sale")


class SaleOrderMessage(models.Model):
    _inherit = "bad.order"

    note = fields.Text(string=_("Note"))
    stage = fields.Selection([(key, _(key)) for key in ("draft", "done")])
    remark = fields.Char(default=lambda self: _("Remark"))

    def action_report(self, partner, *args):
        _("%s ordered %s", partner.name)
        _("%s and %(order)s", partner.name, order=self.name)
        _("Discount must stay below 100 (got %)", partner.active)
        _("Too large: %(size)s", partner.id)
        _("%s ordered", name=partner.name)
        _("%(partner)s ordered %(order)s", partner=partner.name)
        _lt("%s and %s", partner.name, _module="bad_sale")
        _("100% sure")
        _("%s ordered %s", partner.name, self.name)
        _("%(partner)s ordered", partner=partner.name, order=self.name)
        _("%s ordered %s", *args)

        class Summary:
            title = _("Summary")

        return Summary
