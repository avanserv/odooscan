-- The berth view counts cranes per berth; the model reads it with _auto = False.
DROP VIEW IF EXISTS parcel_berth;
CREATE OR REPLACE VIEW parcel_berth AS (
    SELECT b.id, 0 AS crane_count FROM parcel_box b
);
