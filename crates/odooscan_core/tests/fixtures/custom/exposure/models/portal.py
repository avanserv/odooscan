from odoo import fields, models


class ExposureTokenMixin(models.AbstractModel):
    _name = "exposure.token.mixin"
    _description = "Mixin whose fields its models share"

    mixin_token = fields.Char()
    mixin_body = fields.Html(sanitize=False)


class ExposureTicket(models.Model):
    _name = "exposure.ticket"
    _inherit = ["exposure.token.mixin"]
    _description = "Ticket a portal user writes"

    name = fields.Char()
    partner_id = fields.Many2one("res.partner")
    body = fields.Html(sanitize=False)
    reply = fields.Html(sanitize="email_outgoing")
    summary = fields.Html(sanitize_attributes=False)
    edited_body = fields.Html(sanitize_tags=False, compute="_compute_edited_body", inverse="_inverse_edited_body")
    safe_body = fields.Html()
    sent_body = fields.Html(sanitize="email_outgoing", sanitize_tags=True, sanitize_attributes=True)
    staff_body = fields.Html(sanitize=False, groups="base.group_user")
    copied_body = fields.Html(related="body")
    shown_body = fields.Html(sanitize=False, compute="_compute_shown_body")
    api_token = fields.Char()
    webhook_secret = fields.Text()
    password = fields.Char()
    partner_api_key = fields.Char()
    access_token = fields.Char()
    signature = fields.Char()
    retry_token = fields.Integer()
    admin_token = fields.Char(groups="base.group_user")
    copied_token = fields.Char(related="api_token")
    shown_secret = fields.Char(compute="_compute_shown_secret")

    def _compute_edited_body(self):
        for ticket in self:
            ticket.edited_body = ticket.body

    def _inverse_edited_body(self):
        for ticket in self:
            ticket.body = ticket.edited_body

    def _compute_shown_body(self):
        for ticket in self:
            ticket.shown_body = ticket.body

    def _compute_shown_secret(self):
        for ticket in self:
            ticket.shown_secret = ticket.webhook_secret


class ExposureWizard(models.TransientModel):
    _name = "exposure.wizard"
    _description = "Wizard a portal user fills"

    body = fields.Html(sanitize=False)
    api_token = fields.Char()


class ExposureInternal(models.Model):
    _name = "exposure.internal"
    _inherit = ["exposure.token.mixin"]
    _description = "Record only employees reach here"

    body = fields.Html(sanitize=False)
    api_token = fields.Char()


class ExposureBoard(models.Model):
    _name = "exposure.board"
    _description = "Board a portal user reads"

    body = fields.Html(sanitize=False)


class ExposureClosed(models.Model):
    _name = "exposure.closed"
    _description = "Model whose only row grants nothing"

    api_token = fields.Char()


class ExposureOpen(models.Model):
    _name = "exposure.open"
    _description = "Model a row with no group opens to every user"

    api_token = fields.Char()


class ExposureEveryone(models.Model):
    _name = "exposure.everyone"
    _description = "Model whose row names a group 18.0 lacks"

    api_token = fields.Char()
