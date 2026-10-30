import sys

from odoo import api, models
from odoo.exceptions import UserError
from odoo.http import request


class ResPartner(models.Model):
    _inherit = "res.partner"

    def x_try_sync(self):
        try:
            self._x_push_to_crm()
        except UserError:
            self.env.cr.rollback()
            return False
        return True

    @api.model
    def _x_reset(self, cr):
        cr.rollback()

    def x_sync_in_batches(self):
        for partner in self:
            try:
                partner._x_push_to_crm()
                self.env.cr.commit()
            except UserError:
                self.env.cr.rollback()

    def x_sync_apart(self):
        with self.env.registry.cursor() as cr:
            cr.execute("SELECT 1")
            cr.rollback()

    def x_sync_on_new_cursor(self):
        cr = self.env.registry.cursor()
        cr.rollback()
        cr.close()

    def x_sync_or_fail(self):
        try:
            self._x_push_to_crm()
        except UserError:
            self.env.cr.rollback()
            raise

    def x_sync_from_script(self):
        try:
            self._x_push_to_crm()
        except UserError:
            self.env.cr.rollback()
            sys.exit(1)

    def x_sync_writable(self):
        if self.env.cr.readonly:
            self.env.cr.rollback()
        self._x_push_to_crm()

    def x_sync_in_own_env(self):
        with self.env.registry.cursor() as crm_cr:
            crm_env = api.Environment(crm_cr, self.env.uid, {})
            crm_env["res.partner"].browse(self.ids)._x_push_to_crm()
            crm_env.cr.rollback()

    def x_sync_as_own_records(self):
        with self.env.registry.cursor() as crm_cr:
            self = self.with_env(self.env(cr=crm_cr))
            self._x_push_to_crm()
            self.env.cr.rollback()

    def _handle_error(self, exception):
        self.env.cr.rollback()
        return exception

    def _x_push_to_crm(self):
        return True


class IrHttp(models.AbstractModel):
    _inherit = "ir.http"

    @classmethod
    def _handle_error(cls, exception):
        response = super()._handle_error(exception)
        request.env.cr.rollback()
        return response

    @classmethod
    def _handle_exception(cls, exception):
        request.env.cr.rollback()
        return exception
