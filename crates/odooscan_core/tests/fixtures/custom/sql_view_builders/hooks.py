from odoo.tools import SQL


def x_create_named_views(env, names):
    for name in names:
        env.cr.execute(SQL("CREATE OR REPLACE VIEW %s AS (SELECT 1 AS id)", SQL.identifier(name)))
