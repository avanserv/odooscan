from functools import partial, partialmethod
from warnings import catch_warnings

from odoo import api, fields, models, tools

STRICT = True
LOOSE = False


def stamped(method):
    return method


class MethodTicket(models.Model):
    _name = "method.ticket"
    _description = "Method ticket"

    name = fields.Char()
    state = fields.Char()
    weight = fields.Integer(compute="_compute_weight")
    limit = 3

    def _set_state(self, state):
        self.state = state

    def _set_weight(self, weight):
        self.weight = weight

    def _lookup(self):
        return self.ids

    _compute_weight = partialmethod(_set_weight, 1)
    action_done = partialmethod(_set_state, "done")
    action_reopen = partialmethod(_set_state, "open")
    action_partial = partial(_set_state, state="draft")
    action_model = api.model(_lookup)
    action_cached = tools.ormcache("self")(_lookup)
    action_stamped = stamped(_lookup)
    action_either = _lookup if STRICT else _set_state

    if STRICT:

        def action_if(self):
            return True

        def action_settle(self):
            return True

        action_guarded = partialmethod(_set_state, "guarded")
    elif LOOSE:

        def action_elif(self):
            return True

    else:

        def action_else(self):
            return True

    try:

        def action_try(self):
            return True

    except ImportError:

        def action_except(self):
            return True

    else:

        def action_try_else(self):
            return True

    with catch_warnings():
        if STRICT:

            def action_with(self):
                return True


class MethodTicketExtended(models.Model):
    _inherit = "method.ticket"

    def action_reopen(self):
        return super().action_reopen()

    def action_settle(self):
        return super().action_settle()
