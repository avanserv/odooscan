from psycopg2 import IntegrityError

from odoo.tests import SavepointCase


class TestLedgerWrites(SavepointCase):
    def test_ledger_expected_to_fail(self):
        with self.assertRaises(IntegrityError):
            self.env.cr.execute("UPDATE res_partner SET stage = NULL WHERE id = %s", [self.env.user.partner_id.id])
