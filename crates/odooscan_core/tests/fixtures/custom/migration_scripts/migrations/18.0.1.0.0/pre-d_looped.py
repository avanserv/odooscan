for _stage in ("pre",):

    def migrate(cr, version):
        """Defined in a loop, which may run it any number of times."""
