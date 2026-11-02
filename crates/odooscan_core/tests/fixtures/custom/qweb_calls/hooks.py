from odoo.tools import convert_file


def x_load_code_pages(env):
    convert_file(env, "qweb_calls", "data/code_pages.xml", {}, mode="init")
