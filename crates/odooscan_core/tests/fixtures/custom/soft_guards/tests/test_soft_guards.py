from odoo.tests import TransactionCase

SIDE_RECORD = "soft_guards_side.thing"


class TestSoftGuards(TransactionCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        cls.side_installed = "soft_guards_side" in cls.env["ir.module.module"]._installed()
        if "soft.thing" not in cls.env:
            cls.skipTest(cls, "needs soft_guards_side")
        cls.things = cls.env["soft.thing"].search([("code", "=", "a")])

    def test_held_on_the_class(self):
        if self.side_installed:
            self.assertTrue(self.env["soft.thing"].search([("code", "=", "b")]))

    def test_skipped(self):
        if "soft.thing" not in self.env:
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "c")]))

    def test_skipped_on_a_missing_ref(self):
        try:
            self.env.ref(SIDE_RECORD)
        except ValueError:
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "d")]))

    def test_skipped_without_the_model(self):
        if "soft.absent" not in self.env:
            self.skipTest("needs a module that defines it")
        self.assertTrue(self.env["soft.absent"].search([]))

    def test_unguarded_missing_model(self):
        self.assertTrue(self.env["soft.absent"].search([]))

    def test_skipped_on_something_else(self):
        if not self.env.user.active:
            self.skipTest("needs an active user")
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "other skip")]))

    def test_caught_without_skipping(self):
        try:
            self.env.ref(SIDE_RECORD)
        except ValueError:
            pass
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "no skip")]))

    def test_held_after_a_method_clears_it(self):
        self.side_installed = False
        if self.side_installed:
            self.assertTrue(self.env["soft.thing"].search([("code", "=", "e")]))

    def test_skipped_when_present(self):
        if "soft.thing" in self.env:
            self.skipTest("runs without soft_guards_side")
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "inverted skip")]))

    def test_skipped_on_a_call_that_looks_nothing_up(self):
        try:
            self.env.user.check_access("read")
        except ConnectionError:
            self.skipTest("offline")
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "no lookup")]))

    def test_skipped_from_one_handler_of_two(self):
        try:
            self.env.ref(SIDE_RECORD)
        except ValueError:
            self.skipTest("needs soft_guards_side")
        except KeyError:
            pass
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "one handler")]))
