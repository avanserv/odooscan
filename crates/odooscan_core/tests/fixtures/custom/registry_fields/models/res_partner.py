from odoo import fields, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("site", "Site")])


class ResPartnerDepot(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("depot", "Depot")], ondelete={"depot": "set null"})


class ResPartnerPickup(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("pickup", "Pickup")], ondelete={"pickup": "cascade"})


class ResPartnerOrder(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("invoice",), ("contact",)])


class ResPartnerReinvoice(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("invoice",)], ondelete={"invoice": "set null"})


class ResPartnerRelist(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(
        [("contact", "Contact"), ("invoice", "Invoice"), ("courier", "Courier")],
        selection_add=[("courier",)],
    )


class ResPartnerRelistDrop(models.Model):
    _inherit = "res.partner"

    type = fields.Selection([("contact", "Contact"), ("invoice", "Invoice")], selection_add=[("site",), ("locker",)])


class ResPartnerNarrow(models.Model):
    _inherit = "res.partner"

    type = fields.Selection([("contact", "Contact"), ("invoice", "Invoice")])


class ResPartnerNarrowAdd(models.Model):
    _inherit = "res.partner"

    type = fields.Selection(selection_add=[("locker",)])


class ResPartnerGrade(models.Model):
    _inherit = "res.partner"

    x_grade = fields.Selection([("bronze", "Bronze")], required=True)
    x_tier = fields.Selection([("low", "Low")], required=True)
    x_rank = fields.Selection([("first", "First")], selection_add=[("second",)], required=True)


class ResPartnerKiosk(models.Model):
    _inherit = "res.partner"

    type = fields.Selection("_x_partner_types", selection_add=[("kiosk",)])

    def _x_partner_types(self):
        return [("contact", "Contact"), ("invoice", "Invoice"), ("kiosk", "Kiosk")]


class ResPartnerParcel(models.Model):
    _inherit = "res.partner"

    type = fields.Selection((("contact", "Contact"), ("invoice", "Invoice")), selection_add=[("parcel",)])
