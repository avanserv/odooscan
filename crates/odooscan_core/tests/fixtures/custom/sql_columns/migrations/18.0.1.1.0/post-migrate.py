def migrate(cr, version):
    cr.execute("SELECT b.weight_uom FROM parcel_box b")
