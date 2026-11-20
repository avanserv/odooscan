from odoo.addons.domain_bases.models.domain_models import DomainStamped


class DomainStamped(DomainStamped):
    _inherit = "domain.stamped"

    def x_lookup(self):
        return self.search([("no_side_stamped", "=", "a")])
