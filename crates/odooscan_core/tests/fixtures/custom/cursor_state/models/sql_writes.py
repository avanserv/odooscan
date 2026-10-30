from odoo import api, fields, models
from odoo.tools import SQL
from odoo.tools.sql import create_column


def x_reset_ranks(cr):
    cr.execute("UPDATE res_partner SET x_rank = NULL")


class ResPartner(models.Model):
    _inherit = "res.partner"

    x_rank = fields.Integer()
    x_nickname = fields.Char(compute="_x_compute_nickname", inverse="_x_store_nickname")
    x_badge = fields.Char(compute="_x_compute_badge", inverse="_x_store_badge")

    def x_bump_rank(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = x_rank + 1 WHERE id IN %s", [tuple(self.ids)])

    def x_purge_notes(self):
        self.env.cr.execute(SQL("DELETE FROM x_partner_note WHERE partner_id = %s", self.id))

    def x_log_visit(self):
        self.env.cr.execute("INSERT INTO x_partner_visit (partner_id) VALUES (%s)" % self.id)

    def x_rank_from_comment(self):
        self.env.cr.execute("-- rank refresh\nUPDATE res_partner SET x_rank = 0 WHERE x_rank < 0")

    def x_clear_ranks(self):
        self.env.execute_query(SQL("UPDATE res_partner SET x_rank = NULL WHERE id = %s", self.id))

    def x_rank_flushed(self):
        self.env["res.partner"].flush_model(["x_rank"])
        self.env.cr.execute("UPDATE res_partner SET x_rank = 1 WHERE x_rank IS NULL")

    def x_rank_in_closure(self):
        def reset():
            self.env.cr.execute("UPDATE res_partner SET x_rank = 3 WHERE x_rank IS NULL")

        reset()

    def x_rank_model(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 4 WHERE x_rank = 3")
        self.env["res.partner"].invalidate_model(["x_rank"])

    def x_rank_records(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 5 WHERE x_rank = 4")
        self.invalidate_recordset(["x_rank"])

    def x_rank_everything(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 6 WHERE x_rank = 5")
        self.env.invalidate_all()

    def x_rank_legacy(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 7 WHERE x_rank = 6")
        self.invalidate_cache(["x_rank"])

    def x_rank_private(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 8 WHERE x_rank = 7")
        self._invalidate_cache(["x_rank"])

    def x_rank_cache(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 9 WHERE x_rank = 8")
        self.env.cache.invalidate([(self._fields["x_rank"], None)])

    def x_rank_then_refresh(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 10 WHERE x_rank = 9")
        self._x_refresh_ranks()

    def _x_refresh_ranks(self):
        self.env["res.partner"].invalidate_model(["x_rank"])

    def x_read_ranks(self):
        self.env.cr.execute("SELECT id FROM res_partner WHERE x_rank > 0")
        return self.env.cr.fetchall()

    def x_rank_any_table(self):
        self.env.cr.execute(SQL("UPDATE %s SET x_rank = 0", SQL.identifier(self._table)))
        self.env.cr.execute(f"UPDATE {self._table} SET x_rank = 0")

    def x_rank_with_cte(self):
        self.env.cr.execute("WITH low AS (SELECT id FROM res_partner) UPDATE res_partner SET x_rank = 0")

    def x_rank_from_variable(self):
        query = "UPDATE res_partner SET x_rank = 11"
        self.env.cr.execute(query)

    def init(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 0 WHERE x_rank IS NULL")

    def _auto_init(self):
        result = super()._auto_init()
        self.env.cr.execute("UPDATE res_partner SET x_rank = 0 WHERE x_rank < 0")
        return result

    def _init_column(self, column_name):
        if column_name != "x_rank":
            return super()._init_column(column_name)
        self.env.cr.execute("UPDATE res_partner SET x_rank = 1")
        return None

    @api.autovacuum
    def _gc_x_rank(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 0 WHERE active IS NOT TRUE")

    def x_rank_apart(self):
        with self.env.registry.cursor() as cr:
            cr.execute("UPDATE res_partner SET x_rank = 13 WHERE id = %s", [self.id])

    def _x_compute_nickname(self):
        for partner in self:
            partner.x_nickname = partner.name

    def _x_store_nickname(self):
        for partner in self:
            self.env.cr.execute("UPDATE res_partner SET name = %s WHERE id = %s", [partner.x_nickname, partner.id])

    def _x_compute_badge(self):
        for partner in self:
            partner.x_badge = partner.ref

    def _x_store_badge(self):
        for partner in self:
            self.env.cr.execute("UPDATE res_partner SET x_badge = %s WHERE id = %s", [partner.x_badge, partner.id])

    def _reflect_x_rank(self):
        self.env.cr.execute("UPDATE res_partner SET x_rank = 14 WHERE x_rank IS NULL")

    def x_add_tier(self):
        create_column(self.env.cr, "res_partner", "x_tier", "int4")
        self.env.cr.execute("UPDATE res_partner SET x_tier = 1")

    def x_add_company_tier(self):
        create_column(self.env.cr, "res_company", "x_tier", "int4")
        self.env.cr.execute("UPDATE res_partner SET x_tier = 2")

    def x_add_tier_set_rank(self):
        create_column(self.env.cr, "res_partner", "x_tier", "int4")
        self.env.cr.execute("UPDATE res_partner SET x_rank = 2")

    def x_rank_apart_env(self):
        with self.env.registry.cursor() as cr:
            env = api.Environment(cr, self.env.uid, {})
            env.cr.execute("UPDATE res_partner SET x_rank = 16 WHERE id = %s", [self.id])

    def x_rank_logged(self):
        cr = self.env.cr

        def log(message):
            with self.env.registry.cursor() as cr:
                cr.execute("INSERT INTO x_partner_log (message) VALUES (%s)", [message])

        cr.execute("UPDATE res_partner SET x_rank = 17 WHERE x_rank = 16")
        log("ranks moved")

    def x_rank_batches(self):
        class Batch:
            def __init__(self, cr):
                self.cr = cr

            def reset(self):
                self.cr.execute("UPDATE res_partner SET x_rank = 18 WHERE x_rank IS NULL")

        Batch(self.env.cr).reset()

    def x_rank_query_extended(self):
        query = "UPDATE res_partner SET x_rank = 19"
        query += " WHERE x_rank IS NULL"
        self.env.cr.execute(query)

    def x_rank_query_rebound(self):
        query = "UPDATE res_partner SET x_rank = 20"
        if self.env.context.get("x_all_ranks"):
            query = "UPDATE res_partner SET x_rank = 21"
        self.env.cr.execute(query)

    def x_purge_notes_counted(self):
        self.env.cr.execute(
            "WITH gone AS (DELETE FROM x_partner_note WHERE partner_id = %s RETURNING id) SELECT count(*) FROM gone",
            [self.id],
        )

    def x_lock_ranked(self):
        self.env.cr.execute(
            "WITH locked AS (SELECT id FROM res_partner WHERE x_rank > 0 FOR UPDATE SKIP LOCKED) SELECT id FROM locked"
        )

    def x_rank_maybe_apart(self, apart=False):
        if apart:
            cr = self.env.registry.cursor()
            self = self.with_env(self.env(cr=cr))
            self.env.cr.execute("UPDATE res_partner SET x_rank = 22 WHERE id = %s", [self.id])
        self.env.cr.execute("UPDATE res_partner SET x_rank = 23 WHERE x_rank = 22")

    def x_rank_only_apart(self, apart=False):
        if apart:
            self = self.with_env(self.env(cr=self.env.registry.cursor()))
        else:
            return
        self.env.cr.execute("UPDATE res_partner SET x_rank = 24 WHERE x_rank = 23")

    def x_rank_retried_apart(self, attempts):
        for _attempt in range(attempts):
            self = self.with_env(self.env(cr=self.env.registry.cursor()))
        self.env.cr.execute("UPDATE res_partner SET x_rank = 25 WHERE x_rank = 24")

    def x_add_tier_purge(self):
        create_column(self.env.cr, "res_partner", "x_tier", "int4")
        self.env.cr.execute("DELETE FROM res_partner WHERE x_tier IS NULL AND active IS NOT TRUE")

    def x_add_tier_history(self):
        create_column(self.env.cr, "x_partner_tier", "x_level", "int4")
        self.env.cr.execute("INSERT INTO x_partner_tier (x_level) VALUES (1)")
