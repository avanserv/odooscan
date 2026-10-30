-- Seeds one box, so that a new database can create shipments at once.
INSERT INTO parcel_box (id) VALUES (1) ON CONFLICT DO NOTHING;
