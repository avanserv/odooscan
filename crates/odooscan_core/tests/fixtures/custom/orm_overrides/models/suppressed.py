import contextlib

from odoo import api, fields, models


class ResPartnerHandleHints(models.Model):
    _inherit = "res.partner"

    handle_digits = fields.Char(compute="_compute_handle_digits")
    handle_country = fields.Char(compute="_compute_handle_country")

    @api.depends("handle")
    def _compute_handle_digits(self):
        for partner in self:
            with contextlib.suppress(ValueError):
                partner.handle_digits = False
                self._fill_handle_digits()
            if partner.type == "invoice":
                return

    @api.depends("handle")
    def _compute_handle_country(self):
        for partner in self:
            while True:
                with contextlib.suppress(ConnectionError):
                    self._fill_handle_country()
                    break
            partner.handle_country = partner.handle[:2] if partner.handle else False
            if partner.type == "invoice":
                return

    def _fill_handle_digits(self):
        for partner in self:
            partner.handle_digits = "".join(c for c in partner.handle or "" if c.isdigit()) or False

    def _fill_handle_country(self):
        for partner in self:
            partner.handle_country = partner.country_id.code or False
