from odoo import models

from .hook_steps import HookParts


class HiddenHook(models.Model, HookParts):
    _name = "hidden.hook"
    _description = "Hidden hook"
