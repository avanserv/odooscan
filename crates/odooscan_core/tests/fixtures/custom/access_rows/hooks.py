from odoo.tools import convert_file


def x_load_portal_access(env):
    convert_file(env, "access_rows", "security/ir.model.access-hook.csv", {}, mode="init")
