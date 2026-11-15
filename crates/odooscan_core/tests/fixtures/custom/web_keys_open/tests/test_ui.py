from odoo.tests import HttpCase


class TestUi(HttpCase):
    def test_tour(self):
        self.start_tour("/odoo", "web_keys_open_built")
