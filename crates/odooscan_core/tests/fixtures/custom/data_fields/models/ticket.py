from odoo import api, fields, http, models
from odoo.tools import SQL


class DataTicket(models.Model):
    _name = "data.ticket"
    _description = "Ticket"
    _order = "x_text"
    _rec_name = "x_text"
    _active_name = "x_active"
    _parent_name = "x_parent_id"
    _fold_name = "x_checked"
    _rec_names_search = ["x_text"]

    name = fields.Char()
    amount = fields.Monetary()
    label = fields.Char(compute="_compute_label")
    depth = fields.Integer(compute="_compute_depth")
    x_python = fields.Char()
    own_related = fields.Char(related="x_text")
    own_wrong_type = fields.Integer(related="x_partner_id.name")
    own_line_ids = fields.One2many("data.ticket.line", "x_ticket_id")
    own_reference = fields.Many2oneReference(model_field="x_res_model")
    other_id = fields.Many2one("data.ticket", domain=[("x_text", "=", "a")])

    _x_checked_positive = models.Constraint("CHECK(x_checked > 0)", "Positive")

    @api.depends("x_depended")
    def _compute_label(self):
        for ticket in self:
            ticket.label = ticket.name

    @api.depends("x_parent_id.depth")
    def _compute_depth(self):
        for ticket in self:
            ticket.depth = ticket.x_parent_id.depth + 1

    @api.constrains("x_text")
    def _check_text(self):
        return True

    @api.onchange("x_text")
    def _onchange_text(self):
        return None

    def _x_columns(self):
        return self.env.execute_query(SQL("SELECT t.x_column FROM data_ticket t"))

    def _x_demo_columns(self):
        return self.env.execute_query(SQL("SELECT t.x_demo_column FROM data_ticket t"))

    def _x_load_extra(self):
        return "data/mentioned.xml"


class DataTicketLine(models.Model):
    _name = "data.ticket.line"
    _description = "Ticket line"

    name = fields.Char()
    ticket_id = fields.Many2one("data.ticket")

    def _auto_init(self):
        result = super()._auto_init()
        self.env.cr.execute("UPDATE data_ticket t SET name = t.x_column")
        return result


class DataTicketCopy(models.Model):
    _name = "data.ticket.copy"
    _description = "Ticket copy"
    _inherits = {"data.ticket": "ticket_id"}

    ticket_id = fields.Many2one("data.ticket", required=True, ondelete="cascade")


class DataTicketReport(models.Model):
    _name = "data.ticket.report"
    _description = "Ticket report"
    _auto = False

    name = fields.Char()

    def init(self):
        self.env.cr.execute(
            "CREATE OR REPLACE VIEW data_ticket_report AS (SELECT t.id, t.x_column AS name FROM data_ticket t)"
        )


class DataTicketCopyColumns(models.Model):
    _inherit = "data.ticket.copy"

    _x_column_index = models.Index("(x_column)")
    _x_copy_note_index = models.Index("(x_copy_note)")

    def _x_copy_columns(self):
        return self.env.execute_query(SQL("SELECT c.x_column, c.x_copy_note FROM data_ticket_copy c"))


class DataTicketBase(models.Model):
    _name = "data.ticket.base"
    _description = "Ticket base"

    kind = fields.Selection([("open", "Open")])


class DataTicketMid(models.Model):
    _name = "data.ticket.mid"
    _description = "Ticket mid"
    _inherits = {"data.ticket.base": "base_id"}

    base_id = fields.Many2one("data.ticket.base", required=True, ondelete="cascade")


class DataTicketLeaf(models.Model):
    _name = "data.ticket.leaf"
    _description = "Ticket leaf"
    _inherits = {"data.ticket.mid": "mid_id"}

    mid_id = fields.Many2one("data.ticket.mid", required=True, ondelete="cascade")

    _kind_index = models.Index("(kind)")

    def _leaf_kinds(self):
        return self.env.execute_query(SQL("SELECT l.kind FROM data_ticket_leaf l"))


class DataTicketHelper(http.Controller):
    def init(self):
        self.env.cr.execute("SELECT t.x_column FROM data_ticket t")


class DataTicketSetup(models.Model):
    _inherit = "data.ticket"

    # Odoo runs _init_column and init_storage only on a table that holds rows, as on the
    # upgrade that adds this field.
    seeded = fields.Char(init_storage="_init_seeded")

    def init(self):
        self._x_setup_helper()

    def _x_setup_helper(self):
        self.env.cr.execute("SELECT t.x_column FROM data_ticket t")

    def _init_seeded(self, records):
        self.env.cr.execute("SELECT t.x_column FROM data_ticket t")

    def _init_column(self, column_name):
        self.env.cr.execute("SELECT t.x_column FROM data_ticket t")
