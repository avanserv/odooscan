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
