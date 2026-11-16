from odoo import _, models


class TranslatedShopShelf(models.AbstractModel):
    _name = "translated_shop.shelf"
    _description = "Shelf"

    def shelf_messages(self, count, crate, shelf):
        return [
            _("%(count)s orders", count=count),
            _("%s crates on %s shelves", count, count),
            _("Shelf %s is full", shelf),
            _("{shelf} was restocked").format(shelf=shelf),
            _("10% off every crate"),
            _("Crates below 5 (got %)"),
            _("%(crate)s moved to %(shelf)s", crate=crate, shelf=shelf),
            _("%s crate", count),
            _("%(count)s shelves", count=count),
        ]
