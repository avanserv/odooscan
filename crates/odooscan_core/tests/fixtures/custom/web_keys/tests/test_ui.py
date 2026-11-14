import unittest

from odoo.tests import HttpCase


class TestUi(HttpCase):
    def test_tours(self):
        self.start_tour("/odoo", "web_keys_checkout", login="admin")
        self.start_tour("/odoo", "web_keys_chekout", login="admin")
        self.start_tour("/odoo", "web_keys_unit_tour")
        self.browser_js("/odoo", "odoo.startTour('web_keys_cart')", "odoo.isTourReady('web_keys_cart')")
        self.browser_js("/odoo", "odoo.startTour('web_keys_' + self.flavour)")
        self.start_tour("/odoo", "web_keys_custom", code="run()", ready="ready()")
        self.start_tour("/odoo", "web_keys_wrapped", **self.options)
        self.start_tour("/odoo", f"web_keys_{self.flavour}")

    def test_side_tour(self):
        if self.env["ir.module.module"]._get("web_keys_side").state != "installed":
            self.skipTest("web_keys_side is not installed")
        self.start_tour("/odoo", tour_name="web_keys_side_tour")

    @unittest.skip("the cart moved to web_keys_side")
    def test_skipped(self):
        self.start_tour("/odoo", "web_keys_cart_old")
