from psycopg2 import IntegrityError

from odoo.tests import TransactionCase
from odoo.tools import mute_logger


class TestSqlWrites(TransactionCase):
    def test_rank_expected_to_fail(self):
        with self.assertRaises(IntegrityError):
            self.env.cr.execute("UPDATE res_partner SET x_rank = 10 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_expected_to_fail_quietly(self):
        with mute_logger("odoo.sql_db"), self.assertRaises(IntegrityError):
            self.env.cr.execute("UPDATE res_partner SET x_rank = 11 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_expected_to_fail_in_a_savepoint(self):
        with self.assertRaises(IntegrityError):
            with self.env.cr.savepoint():
                self.env.cr.execute("UPDATE res_partner SET x_rank = 12 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_after_the_failure(self):
        with self.assertRaises(IntegrityError):
            self.env["res.partner"].create({})
        self.env.cr.execute("UPDATE res_partner SET x_rank = 13 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_in_a_sub_test(self):
        with self.subTest(rank=14):
            self.env.cr.execute("UPDATE res_partner SET x_rank = 14 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_expected_to_fail_with_a_message(self):
        with self.assertRaisesRegex(IntegrityError, "x_rank"):
            self.env.cr.execute("UPDATE res_partner SET x_rank = 15 WHERE id = %s", [self.env.user.partner_id.id])

    def test_rank_in_a_later_call(self):
        with self.assertRaises(IntegrityError):

            def rank():
                self.env.cr.execute("UPDATE res_partner SET x_rank = 16 WHERE id = %s", [self.env.user.partner_id.id])

            self.env["res.partner"].create({})
        rank()

    def test_rank_expected_to_fail_bound(self):
        with self.assertRaises(IntegrityError) as cm:
            self.env.cr.execute("UPDATE res_partner SET x_rank = 17 WHERE id = %s", [self.env.user.partner_id.id])
        self.assertIn("x_rank", str(cm.exception))
