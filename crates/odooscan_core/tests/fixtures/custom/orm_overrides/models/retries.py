import contextlib

from odoo import api, fields, models


class ResPartnerRetryHints(models.Model):
    _inherit = "res.partner"

    retry_label = fields.Char(compute="_compute_retry_label")
    retry_code = fields.Char(compute="_compute_retry_code")
    retry_zone = fields.Char(compute="_compute_retry_zone")
    retry_slot = fields.Char(compute="_compute_retry_slot")

    @api.depends("name")
    def _compute_retry_label(self):
        for partner in self:
            while True:
                try:
                    self._fill_retry_label()
                    break
                except ConnectionError:
                    pass
            partner.retry_label = partner.name or False
            if partner.type == "invoice":
                return

    @api.depends("name")
    def _compute_retry_code(self):
        for partner in self:
            while True:
                partner.retry_code = False
                with contextlib.suppress(ConnectionError):
                    self._fill_retry_code()
                    break
            if partner.type == "invoice":
                return

    @api.depends("name")
    def _compute_retry_zone(self):
        for partner in self:
            attempts = 0
            while True:
                if attempts > 3:
                    partner.retry_zone = False
                    break
                attempts += 1
                with contextlib.suppress(ConnectionError):
                    self._fill_retry_zone()
                    break
            if partner.type == "invoice":
                return

    @api.depends("name")
    def _compute_retry_slot(self):
        for partner in self:
            slots = partner.name.split("-") if partner.name else []
            index = 0
            with contextlib.suppress(IndexError):
                while True:
                    partner.retry_slot = slots[index]
                    index += 1
            if partner.type == "invoice":
                return

    def _fill_retry_label(self):
        for partner in self:
            partner.retry_label = partner.name or False

    def _fill_retry_code(self):
        for partner in self:
            partner.retry_code = partner.name or False

    def _fill_retry_zone(self):
        for partner in self:
            partner.retry_zone = partner.country_id.code or False
