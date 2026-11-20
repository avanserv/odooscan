from odoo import models
from odoo.addons.field_classes.orm.vector import CompanyLink, SubCompanyLink


class Lens(models.Model):
    _inherit = "x.lens"

    company = CompanyLink("Company")
    sub_company = SubCompanyLink("Company")
