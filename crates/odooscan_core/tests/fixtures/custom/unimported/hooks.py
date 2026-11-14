def post_init_hook(env):
    from . import late

    late.setup(env)
