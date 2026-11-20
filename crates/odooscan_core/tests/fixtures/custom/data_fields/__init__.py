from . import models


def _x_pre_init(env):
    env.cr.execute("SELECT t.x_column FROM data_ticket t")


def _x_post_init(env):
    env.cr.execute("SELECT t.x_column FROM data_ticket t")
