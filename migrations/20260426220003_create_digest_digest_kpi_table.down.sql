-- Down: drop digest.digest_digest_kpis table
DROP TABLE IF EXISTS digest.digest_digest_kpis CASCADE;
DROP FUNCTION IF EXISTS digest.digest_digest_kpis_audit_timestamp() CASCADE;
