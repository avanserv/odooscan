REPORT_SUFFIXES = ("daily", "weekly")


def x_create_suffix_views(env):
    for suffix in REPORT_SUFFIXES:
        env.cr.execute(f"CREATE OR REPLACE VIEW x_report_{suffix}_by_day AS (SELECT 1 AS id)")  # odooscan: ignore[OD101]
