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

try:
    from odoo.addons.imports_lib.tools import shelf_label
except ModuleNotFoundError:
    shelf_label = None

with suppress(ModuleNotFoundError):
    from odoo.addons.imports_lib.tools import shelf_color

try:
    import odoo.addons.imports_lib.labels
    from odoo.addons.imports_lib.labels import shelf_tag
except ModuleNotFoundError:
    shelf_tag = None

with suppress(ModuleNotFoundError):
    import odoo.addons.imports_lib.labels
    from odoo.addons.imports_lib.labels import shelf_badge

try:
    from odoo.addons.imports_lib.tools import shelf_margin
except (ModuleNotFoundError, ImportError):
    shelf_margin = 0

try:
    from odoo.addons.project.models.project import Task
except ModuleNotFoundError:
    Task = None
