from odoo.tools import convert_file


def x_load_extras(env):
    convert_file(env, "unlisted_data", "security/ir.model.access-hook.csv", {}, mode="init")
    convert_file(env, "unlisted_data", "extra/hook_badges.xml", {}, mode="init")
