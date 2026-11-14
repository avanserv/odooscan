from odoo.addons.imports_lib.gone import shelf
from odoo.addons.project.models.project import Project


def migrate(cr, version):
    shelf(cr, Project)
