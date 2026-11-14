from contextlib import suppress
from typing import TYPE_CHECKING

try:
    from odoo.addons.project.models.project import Milestone
except ImportError:
    Milestone = None

with suppress(ImportError):
    from odoo.addons.imports_lib.gone import shelf

if TYPE_CHECKING:
    from odoo.addons.project.models.project import Task

    def shelf_type():
        from odoo.addons.imports_lib.models import Gone

        return Gone


with suppress(KeyError):
    from odoo.addons.imports_lib.tools import shelf_height
    from odoo.addons.project.models.project import Project
