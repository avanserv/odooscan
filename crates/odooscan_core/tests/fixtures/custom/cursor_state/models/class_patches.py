from odoo.addons.base.controllers.main import LabelController
from odoo.addons.base.services import LabelQueue
from odoo.addons.base.services.label_service import LabelPrinter
from odoo.addons.base.wizard.x_label_wizard import LabelFormat, LabelWizard
from odoo.addons.x_label_reports.report.label_report import LabelReport

# Model advice: the index knows LabelWizard, and the report package tells LabelReport.
LabelWizard._x_copies = 1
LabelReport._x_copies = 1

# Subclass advice: the index knows LabelFormat is no model, and controllers hold none.
LabelFormat.x_margin = 4
LabelController._x_route = "/labels"

# Model advice from the index alone: services is no model package, and LabelQueue lives in the
# package's __init__.py.
LabelPrinter._x_copies = 1
LabelQueue._x_copies = 1
