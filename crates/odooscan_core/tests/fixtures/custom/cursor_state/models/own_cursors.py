from odoo import api, models


class ResPartner(models.Model):
    _inherit = "res.partner"

    def x_log_apart(self):
        with self.pool.cursor() as cr:
            cr.execute("SELECT 1")
            cr.commit()

    def x_log_in_own_env(self):
        with self.env.registry.cursor() as log_cr:
            env = api.Environment(log_cr, self.env.uid, {})
            env["res.partner"].browse(self.ids)._x_push_to_crm()
            env.cr.commit()

    def x_log_as_own_records(self):
        with self.env.registry.cursor() as cr:
            self = self.with_env(self.env(cr=cr))
            self._x_push_to_crm()
            self.env.cr.commit()

    def x_log_in_steps(self):
        with self.pool.cursor() as cr:

            def step():
                cr.commit()
                cr.rollback()

            step()

    def x_sync_in_steps(self):
        def step():
            self._x_push_to_crm()
            self.env.cr.commit()

        step()

    def x_undo_in_steps(self):
        with self.pool.cursor() as cr:

            def step():
                cr.execute("SELECT 1")
                self.env.cr.rollback()

            step()

    def x_undo_given(self):
        with self.pool.cursor() as cr:

            def step(cr):
                cr.rollback()

            step(self.env.cr)

    def x_log_then_undo(self):
        with self.pool.cursor() as cr:
            cr.commit()
        self.env.cr.rollback()

    def x_sync_on(self, new_cr=None):
        cr = new_cr or self.env.cr
        self._x_push_to_crm()
        cr.commit()

    def x_sync_on_either(self, new_cr=None):
        cr = new_cr if new_cr else self.env.cr
        self._x_push_to_crm()
        cr.commit()
