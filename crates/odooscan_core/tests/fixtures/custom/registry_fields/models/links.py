from odoo import fields, models


class RegistryInvoice(models.Model):
    _inherit = "registry.invoice"

    owner_id = fields.Many2one("res.partner", required=True, ondelete="set null")
    approver_id = fields.Many2one("res.partner", required=True)
    country_id = fields.Many2one("res.country", required=True, ondelete="cascade")
    follower_ids = fields.Many2many("res.partner")
    watcher_ids = fields.Many2many("res.partner")
    reviewer_ids = fields.Many2many("res.partner", "registry_invoice_reviewer_rel", "invoice_id", "partner_id")
    notified_ids = fields.Many2many(
        "res.partner", relation="registry_invoice_partners_notified_about_each_payment_reminder_rel"
    )
    contact_ids = fields.Many2many("res.partner", compute="_compute_contact_ids")
    state = fields.Selection([("draft", "Draft"), ("posted", "Posted")])

    def _compute_contact_ids(self):
        for invoice in self:
            invoice.contact_ids = invoice.partner_id


class RegistryInvoiceApproval(models.Model):
    _inherit = "registry.invoice"

    approver_id = fields.Many2one(ondelete="set null")
    follower_ids = fields.Many2many(string="Followers")
    state = fields.Selection(selection_add=[("sent", "Sent")])


class RegistryChecklist(models.Model):
    _name = "registry.inspection.checklist"
    _description = "Inspection checklist"

    template_ids = fields.Many2many("registry.inspection.checklist.template")


class RegistryChecklistTemplate(models.Model):
    _name = "registry.inspection.checklist.template"
    _description = "Inspection checklist template"

    name = fields.Char()


class RegistryChecklistArchive(models.Model):
    _name = "registry.inspection.checklist.archive"
    _description = "Archived inspection checklist"
    _table = "registry_archive"

    template_ids = fields.Many2many("registry.inspection.checklist.template")


class RegistryShipmentHandler(models.Model):
    _inherit = "registry.shipment"

    handler_id = fields.Many2one(ondelete="cascade")
