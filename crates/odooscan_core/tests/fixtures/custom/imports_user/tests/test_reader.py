from odoo.tests import TransactionCase


class TestReader(TransactionCase):
    def test_project(self):
        from odoo.addons.project.tests.common import TestProjectCommon

        self.assertTrue(TestProjectCommon)
