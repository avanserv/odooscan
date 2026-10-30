from odoo import api, fields, models


class ButtonStamp(models.AbstractModel):
    _name = "button.stamp"
    _description = "Button stamp"

    def action_stamp(self):
        return True

    @api.private
    def action_seal(self):
        return True


class ButtonOrder(models.Model):
    _name = "button.order"
    _description = "Button order"
    _inherit = ["button.stamp"]

    name = fields.Char()
    active = fields.Boolean(default=True)
    partner_id = fields.Many2one("res.partner")
    line_ids = fields.One2many("button.line", "order_id")

    def action_confirm(self):
        return True

    action_validate = action_confirm

    def _helper(self):
        return True

    @api.private
    def action_audit(self):
        return True

    def action_seal(self):
        return True


class ButtonLine(models.Model):
    _name = "button.line"
    _description = "Button line"

    name = fields.Char()
    order_id = fields.Many2one("button.order")

    def action_split(self):
        return True


class ButtonTicket(models.Model):
    _name = "button.ticket"
    _description = "Button ticket"
    _inherits = {"button.order": "order_id"}

    order_id = fields.Many2one("button.order", required=True, ondelete="cascade")
