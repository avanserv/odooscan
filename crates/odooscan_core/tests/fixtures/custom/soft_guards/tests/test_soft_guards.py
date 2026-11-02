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

    def test_ref_in_a_guarding_try(self):
        try:
            self.assertTrue(self.env.ref("soft_guards_side.in_the_body"))
        except ValueError:
            self.skipTest("needs soft_guards_side")

    def test_ref_after_a_guarding_try(self):
        try:
            self.env.ref("soft_guards_side.looked_up")
        except ValueError:
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_the_try"))

    def test_ref_in_the_else_of_a_guarding_try(self):
        try:
            self.env.ref("soft_guards_side.looked_up")
        except ValueError:
            self.skipTest("needs soft_guards_side")
        else:
            self.assertTrue(self.env.ref("soft_guards_side.in_the_else"))

    def test_ref_under_a_test_of_the_model(self):
        if "soft.thing" in self.env:
            self.assertTrue(self.env.ref("soft_guards_side.under_a_test"))

    def test_ref_after_a_skip(self):
        if "soft.thing" not in self.env:
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_a_skip"))

    def test_lookups_in_a_guarding_try(self):
        try:
            self.assertFalse(self.env["soft.absent"].search([]))
            self.assertTrue(self.env["soft.thing"].search([("code", "=", "in the try")]))
        except KeyError:
            self.skipTest("needs soft_guards_side")

    def test_ref_unguarded(self):
        self.assertTrue(self.env.ref("soft_guards_side.unguarded"))

    def test_ref_in_a_try_that_passes(self):
        try:
            self.env.ref("soft_guards_side.passed")
        except ValueError:
            pass

    def test_ref_in_a_try_with_one_handler_of_two_skipping(self):
        try:
            self.env.ref("soft_guards_side.one_handler")
        except ValueError:
            self.skipTest("needs soft_guards_side")
        except KeyError:
            pass

    def test_ref_in_a_handler(self):
        try:
            self.env.ref(SIDE_RECORD)
        except ValueError:
            self.env.ref("soft_guards_side.in_a_handler")
            self.skipTest("needs soft_guards_side")

    def test_ref_in_finally(self):
        try:
            self.env.ref(SIDE_RECORD)
        except ValueError:
            self.skipTest("needs soft_guards_side")
        finally:
            self.env.ref("soft_guards_side.in_finally")

    def test_ref_before_a_guard(self):
        self.env.ref("soft_guards_side.before_a_guard")
        if "soft.thing" not in self.env:
            self.skipTest("needs soft_guards_side")

    def test_missing_id_under_a_guard(self):
        if "soft.thing" in self.env:
            self.assertTrue(self.env.ref("base.no_such_guarded_id"))

    def test_ref_after_ensure_installed(self):
        self.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_the_helper"))

    def test_ref_of_another_module_after_ensure_installed(self):
        self.ensure_installed("sale")
        self.assertTrue(self.env.ref("soft_guards_side.after_another_helper"))

    def test_lookups_after_ensure_installed(self):
        self.ensure_installed("soft_guards_side")
        self.assertFalse(self.env["soft.absent"].search([]))
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "helper")]))

    @classmethod
    def _ref_from_the_class(cls):
        cls.ensure_installed("soft_guards_side")
        return cls.env.ref("soft_guards_side.after_the_class_helper")

    def test_missing_id_after_ensure_installed(self):
        self.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("base.no_such_id_after_ensure_installed"))

    def test_ref_without_ensure_installed(self):
        self.assertTrue(self.env.ref("soft_guards_side.no_helper"))

    def test_ref_before_ensure_installed(self):
        self.env.ref("soft_guards_side.before_the_helper")
        self.ensure_installed("soft_guards_side")

    def test_ref_after_ensure_installed_of_a_variable(self):
        module = "soft_guards_side"
        self.ensure_installed(module)
        self.assertTrue(self.env.ref("soft_guards_side.after_a_variable"))

    def test_ref_after_a_conditional_ensure_installed(self):
        if self.env.user.active:
            self.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_a_branch"))

    def test_ref_after_ensure_installed_on_another_object(self):
        self.helper.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_another_object"))

    def test_lookups_without_ensure_installed(self):
        self.assertFalse(self.env["soft.absent"].search([]))
        self.assertTrue(self.env["soft.thing"].search([("code", "=", "no helper")]))

    def test_ref_of_another_module_after_ensure_installed_of_an_indexed_one(self):
        self.ensure_installed("base_only")
        self.assertTrue(self.env.ref("soft_guards_side.after_an_unrelated_helper"))

    def test_ref_after_a_test_of_an_unrelated_model(self):
        if "web.palette" not in self.env:
            self.skipTest("needs web")
        self.assertTrue(self.env.ref("soft_guards_side.after_an_unrelated_model"))

    def test_ref_after_a_state_of_an_unrelated_module(self):
        if self.env["ir.module.module"]._get("base_only").state != "installed":
            self.skipTest("needs base_only")
        self.assertTrue(self.env.ref("soft_guards_side.after_an_unrelated_state"))

    def test_heir_ref_after_ensure_installed(self):
        self.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_heir.heir_thing"))

    def test_heir_ref_after_a_test_of_the_model(self):
        if "soft.thing" not in self.env:
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_heir.heir_thing"))

    def test_ref_after_ensure_installed_of_two_modules(self):
        self.ensure_installed("base_only")
        self.ensure_installed("soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_both_helpers"))

    def test_ref_under_a_test_of_the_model_after_ensure_installed(self):
        self.ensure_installed("base_only")
        if "soft.thing" in self.env:
            self.assertTrue(self.env.ref("soft_guards_side.under_a_test_after_a_helper"))

    def test_ref_after_hasattr(self):
        if not hasattr(self.env.user, "thing_ids"):
            self.skipTest("needs soft_guards_side")
        self.assertTrue(self.env.ref("soft_guards_side.after_hasattr"))

    def test_ref_after_a_test_of_a_model_no_module_defines(self):
        if "soft.absent" not in self.env:
            self.skipTest("needs a module that defines it")
        self.assertTrue(self.env.ref("soft_guards_side.after_an_absent_model"))

    def test_ref_after_ensure_installed_of_a_module_with_a_dependency_outside_the_scan(self):
        self.ensure_installed("soft_guards_open")
        self.assertTrue(self.env.ref("soft_guards_side.access_soft_thing"))

    def test_ref_after_a_try_with_a_lookup_that_does_not_raise(self):
        try:
            self.env.ref("soft_guards_side.thing", raise_if_not_found=False)
            self.env.ref("base_only.view_partner_color")
        except ValueError:
            self.skipTest("needs base_only")
        self.assertTrue(self.env.ref("soft_guards_side.after_a_lookup_that_does_not_raise"))

    def test_ref_after_a_try_with_a_lookup_under_a_branch(self):
        try:
            if self.env.user.active:
                self.env.ref("soft_guards_side.thing")
            self.env.ref("base_only.view_partner_color")
        except ValueError:
            self.skipTest("needs base_only")
        self.assertTrue(self.env.ref("soft_guards_side.after_a_lookup_under_a_branch"))
