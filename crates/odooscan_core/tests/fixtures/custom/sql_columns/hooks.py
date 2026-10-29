def x_create_parcel_board(env):
    env.cr.execute("CREATE OR REPLACE VIEW parcel_board AS (SELECT 1 AS id, 0 AS board_count)")
    counts = "SELECT d.id, count(*) AS bay_count FROM parcel_box d GROUP BY d.id"
    env.cr.execute(f"CREATE OR REPLACE VIEW parcel_dock AS ({counts})")  # odooscan: ignore[OD101]
    env.cr.execute("CREATE OR REPLACE VIEW parcel_bay AS " + _x_bay_query(env))  # odooscan: ignore[OD101]
    env.cr.execute("CREATE OR REPLACE VIEW public.parcel_pier AS (SELECT 1 AS id, 2 AS berth_count)")


def _x_bay_query(env):
    return "(SELECT 1 AS id, 4 AS door_count)"
