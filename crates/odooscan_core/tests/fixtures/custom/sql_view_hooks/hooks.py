REPORT_SUFFIXES = ("daily", "weekly")


def x_create_report_views(env):
    for suffix in REPORT_SUFFIXES:
        table = f"x_report_{suffix}"
        env.cr.execute(f"CREATE OR REPLACE VIEW public.{table} AS (SELECT 1 AS id)")  # odooscan: ignore[OD101]
