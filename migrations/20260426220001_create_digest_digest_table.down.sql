-- Down: drop digest.digest_digests table
DROP TABLE IF EXISTS digest.digest_digests CASCADE;
DROP FUNCTION IF EXISTS digest.digest_digests_audit_timestamp() CASCADE;
