-- Down: drop digest.digest_tip_users table
DROP TABLE IF EXISTS digest.digest_tip_users CASCADE;
DROP FUNCTION IF EXISTS digest.digest_tip_users_audit_timestamp() CASCADE;
