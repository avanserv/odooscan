{
    "name": "SQL view hooks by suffix",
    "version": "18.0.1.0.0",
    "depends": ["base"],
    "data": ["security/ir.model.access.csv"],
    "post_init_hook": "x_create_suffix_views",
}
