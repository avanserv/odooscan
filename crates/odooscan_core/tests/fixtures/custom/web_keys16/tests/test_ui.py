from odoo.tests import HttpCase


class TestUi(HttpCase):
    def test_tour(self):
        self.start_tour("/web", "web_keys16_tour")
