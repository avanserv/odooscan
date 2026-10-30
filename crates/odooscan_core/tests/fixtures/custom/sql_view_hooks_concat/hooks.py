REPORT_SUFFIXES = ("daily", "weekly")


def x_create_concat_views(env):
    for suffix in REPORT_SUFFIXES:
        env.cr.execute("CREATE OR REPLACE VIEW x_report_" + suffix + "_by_week AS (SELECT 1 AS id)")  # odooscan: ignore[OD101]
