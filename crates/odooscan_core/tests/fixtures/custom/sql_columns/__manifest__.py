{
    "name": "SQL columns",
    "version": "18.0.1.0.0",
    "depends": ["base"],
    "data": ["security/ir.model.access.csv", "data/parcel_berth_view.sql", "data/parcel_seed.sql"],
    "post_init_hook": "x_create_parcel_board",
}
