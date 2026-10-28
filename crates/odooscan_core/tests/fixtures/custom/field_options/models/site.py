from odoo import fields, models


def _site_company_domain(self, companies):
    return [("site_id.company_id", "in", companies)]


class CompanyOwned:
    company_id = fields.Many2one("res.company")


class GreenhouseZone(models.Model):
    _name = "greenhouse.zone"
    _description = "Greenhouse zone"

    company_id = fields.Many2one("res.company")
    country_id = fields.Many2one("res.country")


class GreenhouseBay(models.Model):
    _name = "greenhouse.bay"
    _description = "Greenhouse bay"
    _check_company_domain = _site_company_domain

    site_id = fields.Many2one("greenhouse.site")


class GreenhouseCrate(models.Model, CompanyOwned):
    _name = "greenhouse.crate"
    _description = "Greenhouse crate"


class GreenhouseSite(models.Model):
    _name = "greenhouse.site"
    _description = "Greenhouse site"
    _check_company_auto = True

    company_id = fields.Many2one("res.company")
    country_id = fields.Many2one("res.country", check_company=True)
    zone_id = fields.Many2one("greenhouse.zone", check_company=True)
    bay_id = fields.Many2one("greenhouse.bay", check_company=True)
    crate_id = fields.Many2one("greenhouse.crate", check_company=True)
    zone_country_id = fields.Many2one("res.country", related="zone_id.country_id", check_company=True)
    line_ids = fields.One2many("greenhouse.site.line", "site_id", check_company=True)
    # garden.zone comes from a module outside depends, so OD222 cannot tell whether it has a
    # company_id and stays silent; OD202 reports the missing dependency.
    garden_zone_id = fields.Many2one("garden.zone", check_company=True)


class GreenhouseSiteCountry(models.Model):
    _inherit = "greenhouse.site"

    country_id = fields.Many2one(check_company=True)


class GreenhouseSiteLine(models.Model):
    _name = "greenhouse.site.line"
    _description = "Greenhouse site line"

    site_id = fields.Many2one("greenhouse.site")


class GreenhouseNetwork(models.Model):
    _name = "greenhouse.network"
    _description = "Greenhouse network"

    company_ids = fields.Many2many("res.company")
    country_ids = fields.Many2many("res.country", check_company=True)


class GreenhouseShelf(models.Model):
    _name = "greenhouse.shelf"
    _description = "Greenhouse shelf"

    country_id = fields.Many2one("res.country", check_company=True)
    origin_id = fields.Many2one("res.country", company_dependent=True, check_company=True)


class ResCompany(models.Model):
    _inherit = "res.company"

    home_country_id = fields.Many2one("res.country", check_company=True)
