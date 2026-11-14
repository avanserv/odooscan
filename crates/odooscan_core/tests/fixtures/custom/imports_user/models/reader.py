from odoo import models
from odoo.addons.base.maintenance.migrations import util
from odoo.addons.imports_lib.models import PartnerShelf, PartnerShelves
from odoo.addons.imports_lib.models import shelving
from odoo.addons.imports_lib.open import format_shelf as open_format
from odoo.addons.imports_lib.tools import SHELF_WIDTH, format_shelf, shelf_depth
from odoo.addons.imports_lib.wizard import shelf_wizard
from odoo.addons.project.models.project import Task
from odoo.addons.web.models import res_partner
from odoo.addons import project as project_addon

from ...project.models import project
from ... import project as project_root


class PartnerReader(models.Model):
    _inherit = "res.partner"

    def _shelf_label(self):
        from odoo.addons.imports_lib.tools import format_label
        from odoo.addons.project.models.project import Project

        return format_label(shelving.shelf_code(self), SHELF_WIDTH)
