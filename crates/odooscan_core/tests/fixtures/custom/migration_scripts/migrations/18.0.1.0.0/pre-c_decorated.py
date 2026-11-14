import functools


@functools.cache
def migrate(env, version):
    """Wrapped, so the signature Odoo reads is not this one."""
