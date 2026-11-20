from psycopg2 import IntegrityError

from odoo.tests import TransactionCase


class TestMemoWrites(TransactionCase):
    def test_memo_expected_to_fail_with_a_message(self):
        with self.assertRaisesRegex(IntegrityError, "ref"):
            self.env.cr.execute("UPDATE res_partner SET ref = NULL WHERE id = %s", [self.env.user.partner_id.id])
