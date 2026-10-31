from openerp import models


class ResCompany(models.Model):
    _inherit = "res.company"

    def _auto_end(self, cr, context=None):
        return super(ResCompany, self)._auto_end(cr, context=context)

    def _read_group_get_domain(self, groupby, value):
        return super(ResCompany, self)._read_group_get_domain(groupby, value)

    def _auto_finish(self):
        return super(ResCompany, self)._auto_finish()
