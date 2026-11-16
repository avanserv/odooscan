import re
import uuid

from odoo import models


class SaleOrderQueries(models.Model):
    _inherit = "bad.order"

    MAIL_PATTERN = re.compile(r"[^@\s]+@[^@\s]+")

    def action_close_lines(self):
        lines = self.line_ids
        for line in lines:
            line.write({"amount": 0.0})

    def action_queries(self):
        default = self.env["res.partner"].browse()
        for order in self:
            self.env["res.partner"].search([("name", "=", "Main")])
            self.env["bad.order.line"].search_count([("order_id", "=", order.id)])
            self.env["bad.order.line"].search([("order_id", "=", order.id)])
            self.env["res.partner"].browse(order.partner_id.id).name
            self.env["bad.order.line"].create({"order_id": order.id})
            order.sudo().write({"name": "Closed"})
            order.write({"name": order.partner_id.name})
            self.env["bad.order.line"].search([("order_id", "=", order.id)], limit=1)
            domain = [("order_id", "=", order.id)]
            self.env["bad.order.line"].search(domain)
            self.env[order._name].search([])
            line = self.env["bad.order.line"].create({"order_id": order.id})
            line.amount = 1.0
            try:
                self.env["res.partner"].search([("name", "=", "Main")])
            except ValueError:
                pass
            if not default:
                default = self.env["res.partner"].search([("name", "=", "Default")], limit=1)
            self.env["res.partner"].browse(order.partner_id.ids)

            def later():
                return self.env["res.partner"].search([])

        for name in ("a", "b"):
            self.env["res.partner"].search([("name", "=", name)])

    def action_counts(self, text):
        count = len(self.env["bad.order.line"].search([("amount", ">", 0)]))
        first = self.env["bad.order.line"].search([("amount", ">", 0)])[0]
        head = self.search([])[:1]
        one = self.search([], limit=1)[0]
        match = re.search("a", text)[0]
        return count, first, head, one, match

    def action_per_order(self):
        for order in self:
            self.env["bad.order.line"].search([("order_id", "child_of", order.id)])
            try:
                order.write({"name": "Checked"})
            except ValueError:
                order.write({"name": "Failed"})
            if order.name == "Main":
                order.write({"name": "First"})
                break

    def action_numbered(self):
        cache = {}
        for order in self:
            if order.name not in cache:
                cache[order.name] = self.env["res.partner"].search([("name", "=", order.name)])
            order.write({"name": str(uuid.uuid4())})
            order.partner_id = self.env["res.partner"].browse(order.partner_id.id)
            self.env["bad.order.line"].search_count([("order_id.partner_id", "=", order.partner_id.id)])

    def action_sequenced(self, text):
        total = 0
        partner = None
        for order in self:
            code = self.env["ir.sequence"].next_by_code("bad.order")
            self.env["res.partner"].search([("name", "=", code)])
            total = total + self.env["bad.order.line"].search_count([])
            if partner is None:
                partner = self.env["res.partner"].browse(order.partner_id.id)
            order.write({"name": partner.name})
        pattern = self.MAIL_PATTERN
        return total, pattern.search(text)[0]
