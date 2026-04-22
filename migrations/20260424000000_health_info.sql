-- Add health info columns to games table
ALTER TABLE games ADD COLUMN seeders INTEGER;
ALTER TABLE games ADD COLUMN leechers INTEGER;
ALTER TABLE games ADD COLUMN completed INTEGER;
