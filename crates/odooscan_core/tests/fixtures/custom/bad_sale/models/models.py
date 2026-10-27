from odoo import _, api, fields, models


class SaleOrder(models.Model):
    _name = "bad.order"
    _inherit = ["mail.thread", "res.nothing"]
    _sql_constraints = [("name_uniq", "unique(name)", "Name must be unique")]

    name = fields.Char(group_operator="count")
    date = fields.Date(default=fields.Date.today())
    partner_id = fields.Many2one("res.partner")
    country_code = fields.Char(related="partner_id.country_id.cde")
    line_ids = fields.One2many("bad.order.line", "order_id")
    total = fields.Float(compute="_compute_total")
    ghost_id = fields.Many2one("res.ghost")

    @api.depends("line_ids.amount", "partner_id.name.size")
    def _compute_totl(self):
        for order in self:
            order.total = sum(order.line_ids.mapped("amount"))

    @api.multi
    def name_get(self):
        return []

    def write(self, vals):
        self.env.cr.execute("UPDATE bad_order SET name = '%s'" % vals["name"])
        self.env.cr.execute(f"DELETE FROM {self._table} WHERE id = %s", (self.id,))
        self._cr.commit()  # odooscan: ignore[OD103]
        self.env.cr.commit()
        return True

    def action_warn(self):
        name = self.name
        raise ValueError(_("Order %s is late" % name))

    def action_partner(self):
        return self.env["res.partner"].search([]) | self.env["bad.unknown"].browse()


class SaleOrderLine(models.Model):
    _name = "bad.order.line"

    order_id = fields.Many2one("bad.order")
    amount = fields.Float()

    def fields_view_get(self, view_id=None, view_type="form", toolbar=False, submenu=False):
        return {}


class ResPartner(models.Model):
    _inherit = "res.partner"

    name = fields.Text()
    message_ids = fields.One2many("res.partner", "parent_id")
