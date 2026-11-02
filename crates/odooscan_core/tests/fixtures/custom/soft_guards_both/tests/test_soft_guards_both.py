from odoo.tests import TransactionCase


class TestSoftGuardsBoth(TransactionCase):
    def test_heir_ref_without_a_guard(self):
        self.assertTrue(self.env.ref("soft_guards_heir.heir_thing"))

    def test_heir_ref_after_ensure_installed_of_an_unrelated_module(self):
        self.ensure_installed("base_only")
        self.assertTrue(self.env.ref("soft_guards_heir.heir_thing"))

    def test_heir_ref_after_a_test_of_an_unrelated_model(self):
        if "res.partner" not in self.env:
            self.skipTest("needs base")
        self.assertTrue(self.env.ref("soft_guards_heir.heir_thing"))
