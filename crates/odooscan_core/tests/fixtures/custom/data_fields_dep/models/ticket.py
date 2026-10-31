from odoo import api, fields, models


class DataTicket(models.Model):
    _inherit = "data.ticket"
    _rec_name = "x_text"
    _active_name = "x_active"

    dep_amount = fields.Monetary()
    dep_related = fields.Char(related="x_text")
    dep_wrong_type = fields.Integer(related="x_partner_id.name")
    dep_line_ids = fields.One2many("data.ticket.line", "x_ticket_id")
    dep_reference = fields.Many2oneReference(model_field="x_res_model")
    dep_label = fields.Char(compute="_compute_dep_label")
    dep_depth = fields.Integer(compute="_compute_dep_depth")
    dep_demo = fields.Char(related="x_demo_text")

    @api.depends("x_depended")
    def _compute_dep_label(self):
        for ticket in self:
            ticket.dep_label = ticket.name

    @api.depends("x_parent_id.dep_depth")
    def _compute_dep_depth(self):
        for ticket in self:
            ticket.dep_depth = ticket.x_parent_id.dep_depth + 1

    def init(self):
        self.env.cr.execute("UPDATE data_ticket t SET name = t.x_column")
