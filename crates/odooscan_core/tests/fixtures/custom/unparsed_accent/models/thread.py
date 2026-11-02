# -*- coding: utf-8 -*-
# © 2016 Hand-written fixture, no real author
from odoo import models


class UnparsedAccent(models.Model):
    _name = 'unparsed.accent'
    _inherit = ['mail.thread', 'ir.needaction_mixin']



class UnparsedSpelled(models.Model):
    _name = "unparsed.spelled"
    _inherit = ["mail.thread", THREAD_MIXIN]

    def action_note(self):
        print "noted"
